//! The scenarios of the packages (#46) that can run without a push: the version computed from
//! the commits, the changelog, the archive and its sum, `install.sh` and `flottille update`
//! against a local server standing for the releases, in a throwaway home. Nothing here talks to
//! GitHub; what only a real push proves is said in `docs/releases.md`.

use crate::common;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use serde_json::json;
use sha2::{Digest, Sha256};

/// The repository's own scripts.
fn tool(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(name)
}

/// A throwaway git repository, none of the machine's git settings.
struct Repo {
    dir: tempfile::TempDir,
    /// A `gh` standing for GitHub's API (`bin/gh`), and the descriptions of the pull requests it
    /// knows (`prs/<number>`, and beside it the author's association and whether it can be read): the
    /// changelog reads their `Note:` lines through it.
    gh: tempfile::TempDir,
}

impl Repo {
    fn new() -> Repo {
        let repo = Repo {
            dir: tempfile::tempdir().unwrap(),
            gh: tempfile::tempdir().unwrap(),
        };
        repo.git(&["init", "--quiet", "--initial-branch=main"]);
        std::fs::create_dir_all(repo.gh.path().join("bin")).unwrap();
        std::fs::create_dir_all(repo.gh.path().join("prs")).unwrap();
        let gh = repo.gh.path().join("bin/gh");
        // `gh api repos/{owner}/{repo}/pulls/<n> --jq <expression>`: the answer the changelog asks
        // for (the author's association on a line, then the body), or a failure for a pull
        // request marked unreadable. One known by neither is an empty description by an owner.
        std::fs::write(
            &gh,
            "#!/bin/sh\n[ \"$1\" = api ] && [ \"$3\" = --jq ] || exit 2\n\
             prs=$(dirname \"$0\")/../prs n=${2##*/}\n\
             [ ! -e \"$prs/$n.unreadable\" ] || exit 1\n\
             cat \"$prs/$n.association\" 2>/dev/null || echo OWNER\n\
             cat \"$prs/$n\" 2>/dev/null || true\n",
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        repo
    }

    /// The description of pull request `number`, as GitHub's API says it.
    fn pr(&self, number: u32, body: &str) {
        std::fs::write(self.gh.path().join(format!("prs/{number}")), body).unwrap();
    }

    /// Pull request `number` was written by someone who is `association` to the repository
    /// (`OWNER`, `MEMBER`, `COLLABORATOR`, `CONTRIBUTOR`, ...), as GitHub's API says it.
    fn pr_by(&self, number: u32, association: &str) {
        std::fs::write(
            self.gh.path().join(format!("prs/{number}.association")),
            format!("{association}\n"),
        )
        .unwrap();
    }

    /// The API fails to read pull request `number`.
    fn pr_unreadable(&self, number: u32) {
        std::fs::write(self.gh.path().join(format!("prs/{number}.unreadable")), "").unwrap();
    }

    /// The environment that puts the fake `gh` first on the `PATH`.
    fn with_gh<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.gh.path().join("bin")];
        paths.extend(std::env::split_paths(&path));
        command.env("PATH", std::env::join_paths(paths).unwrap())
    }

    fn git_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(self.dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
        command
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.git_command(args).output().unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// A commit with that message, its own file changed.
    fn commit(&self, message: &str) {
        let n = self.git(&["rev-list", "--all", "--count"]);
        std::fs::write(self.dir.path().join(format!("file-{n}")), message).unwrap();
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", message]);
    }

    fn tag(&self, name: &str) {
        self.git(&["tag", name]);
    }

    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    /// The pull request `dev → main` merged by squash, its title saying nothing of the version.
    /// The head of dev it squashed.
    fn squash_dev_into_main(&self) -> String {
        self.git(&["checkout", "--quiet", "dev"]);
        let dev = self.head();
        self.git(&["checkout", "--quiet", "main"]);
        self.git(&["merge", "--quiet", "--squash", "dev"]);
        self.git(&["commit", "--quiet", "-m", "Release (#99)"]);
        dev
    }

    /// A stable release as CI made it before #575: dev squashed into main, `v<version>` on the
    /// squash and the mark `released/v<version>` on the head of dev it squashed. Back on dev.
    fn release(&self, version: &str) {
        let dev = self.squash_dev_into_main();
        self.tag(&format!("v{version}"));
        self.git(&["tag", &format!("released/v{version}"), &dev]);
        self.git(&["checkout", "--quiet", "dev"]);
    }

    /// The release recorded in dev's history as CI recorded it before #575: a merge of the squash
    /// on main that keeps dev's files (`-s ours`), so the next pull request `dev → main` shares it
    /// as a base. On dev.
    fn record_release_in_dev(&self, version: &str) {
        let message = format!("chore(repo): record the {version} release in dev's history");
        self.git(&["merge", "--quiet", "-s", "ours", "-m", &message, "main"]);
    }

    /// The repository's history as the two releases made by squash left it: `v0.1.0`, recorded
    /// in dev, then `v0.1.1` of that record, recorded too. On dev.
    fn released_by_squash_twice(&self) {
        self.commit("feat(core): start");
        self.git(&["checkout", "--quiet", "-b", "dev"]);
        self.commit("feat(gui): add a page (#300)");
        self.release("0.1.0");
        self.commit("fix(cli): a typo (#330)");
        self.record_release_in_dev("0.1.0");
        self.release("0.1.1");
        self.record_release_in_dev("0.1.1");
    }

    /// A release as the `Release` workflow makes it: main moves forward to the head of dev, and
    /// `v<version>` is put on that very commit. Back on dev; the commit released.
    fn release_by_fast_forward(&self, version: &str) -> String {
        let dev = self.head();
        self.git(&["checkout", "--quiet", "main"]);
        self.git(&["merge", "--quiet", "--ff-only", "dev"]);
        assert_eq!(
            self.head(),
            dev,
            "main is dev's commit, no commit of its own"
        );
        self.tag(&format!("v{version}"));
        self.git(&["checkout", "--quiet", "dev"]);
        dev
    }

    /// A release of a maintenance line as the `Release` workflow makes it: the annotated tag
    /// `v<version>` on HEAD, its message naming the line, `stable/X.Y`.
    fn line_release(&self, version: &str) {
        let line = version.rsplit_once('.').unwrap().0;
        self.git(&[
            "tag",
            "-a",
            &format!("v{version}"),
            "-m",
            &format!("stable/{line}"),
        ]);
    }

    /// `tools/changelog.sh` in this repository.
    fn changelog(&self, args: &[&str]) -> String {
        let output = self
            .with_gh(
                Command::new(tool("tools/changelog.sh"))
                    .args(args)
                    .current_dir(self.dir.path())
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_CONFIG_NOSYSTEM", "1"),
            )
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    /// `tools/version.sh` in this repository.
    fn version(&self, args: &[&str]) -> Output {
        Command::new(tool("tools/version.sh"))
            .args(args)
            .current_dir(self.dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }

    fn next(&self, args: &[&str]) -> String {
        let output = self.version(args);
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}

// The version.

#[test]
fn a_feature_before_1_0_bumps_the_patch() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("docs(repo): say more");
    repo.commit("feat(gui): add a page (#12)");
    repo.commit("fix(cli): a typo (#13)");

    assert_eq!(repo.next(&["stable"]), "0.3.1");

    // A feature alone is enough, and it is not the minor.
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.1.1");
    repo.commit("feat(tools): cut long results (#328)");

    assert_eq!(repo.next(&["stable"]), "0.1.2");
}

#[test]
fn a_feature_after_1_0_bumps_the_minor() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v1.2.3");
    repo.commit("docs(repo): say more");
    repo.commit("feat(gui): add a page (#12)");
    repo.commit("fix(cli): a typo (#13)");

    assert_eq!(repo.next(&["stable"]), "1.3.0");
}

#[test]
fn a_fix_or_perf_bumps_the_patch() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("fix(cli): a typo");
    assert_eq!(repo.next(&["stable"]), "0.3.1");

    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v1.2.3");
    repo.commit("perf(store): fewer reads");
    assert_eq!(repo.next(&["stable"]), "1.2.4");
}

#[test]
fn a_breaking_change_bumps_the_major_from_1_0() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v1.4.2");
    repo.commit("feat(cli)!: rename the commands");
    assert_eq!(repo.next(&["stable"]), "2.0.0");

    // Said in the body's footer instead.
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v1.4.2");
    repo.commit("fix(store): a new schema\n\nBREAKING CHANGE: the old store is not read");
    assert_eq!(repo.next(&["stable"]), "2.0.0");
}

#[test]
fn a_breaking_change_before_1_0_bumps_the_minor() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.4");
    repo.commit("fix(cli)!: rename the commands");
    assert_eq!(repo.next(&["stable"]), "0.4.0");

    // A feature that breaks is the minor too, not the patch a feature alone is.
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.4");
    repo.commit("feat(cli)!: rename the commands");
    assert_eq!(repo.next(&["stable"]), "0.4.0");

    // Said in the body's footer instead.
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.4");
    repo.commit("fix(store): a new schema\n\nBREAKING CHANGE: the old store is not read");
    assert_eq!(repo.next(&["stable"]), "0.4.0");
}

#[test]
fn without_a_stable_tag_the_versions_count_from_0_0_0() {
    let repo = Repo::new();
    repo.commit("chore(repo): start");
    repo.commit("feat(core): the first thing");
    assert_eq!(repo.next(&["stable"]), "0.0.1");
    assert_eq!(repo.next(&["beta"]), "0.0.1-beta.2");
}

#[test]
fn without_a_bumping_commit_a_release_is_the_next_patch() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("docs(repo): say more");
    repo.commit("ci(repo): one more check");

    assert_eq!(repo.next(&["beta"]), "0.3.1-beta.2");
    assert_eq!(repo.next(&["stable"]), "0.3.1");
}

#[test]
fn a_rerun_on_main_after_the_release_publishes_nothing() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#14)");
    let dev = repo.head();
    repo.release("0.4.0");

    let output = repo.version(&["stable", "--at", &dev]);

    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("nothing to release since v0.4.0"),
        "{output:?}"
    );
}

#[test]
fn a_beta_is_numbered_after_the_previous_one() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("fix(cli): a typo (#13)");
    assert_eq!(repo.next(&["beta"]), "0.3.1-beta.1");
    repo.commit("feat(gui): add a page (#14)");
    assert_eq!(repo.next(&["beta"]), "0.3.1-beta.2");
    repo.commit("docs(repo): say more (#15)");
    assert_eq!(repo.next(&["beta"]), "0.3.1-beta.3");
}

#[test]
fn pre_release_tags_are_not_stable_ones() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("feat(gui): add a page");
    repo.tag("v0.4.0-beta.1");
    repo.commit("fix(cli): a typo");

    assert_eq!(repo.next(&["stable"]), "0.3.1");
    assert_eq!(repo.next(&["beta"]), "0.3.1-beta.2");
}

#[test]
fn an_alpha_names_its_branch_and_counts_its_commits() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "feat/46-packages"]);
    repo.commit("feat(ci): publish the packages");
    repo.commit("test(ci): the versions");

    assert_eq!(
        repo.next(&["alpha", "--branch", "feat/46-packages"]),
        "0.3.1-alpha.feat-46-packages.2"
    );
    // Its name as git has it when none is given.
    assert_eq!(repo.next(&["alpha"]), "0.3.1-alpha.feat-46-packages.2");
}

#[test]
fn the_stable_version_is_the_beta_of_the_squashed_dev_head_without_its_pre_release() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#14)");
    repo.commit("fix(cli): a typo (#15)");
    assert_eq!(repo.next(&["beta"]), "0.3.1-beta.2");

    let dev = repo.squash_dev_into_main();

    assert_eq!(repo.next(&["stable", "--at", &dev]), "0.3.1");
}

#[test]
fn after_dev_is_squashed_into_main_the_next_beta_counts_from_the_released_mark() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#14)");
    repo.commit("fix(cli): a typo (#15)");
    repo.release("0.3.1");
    repo.commit("fix(store): a lock (#16)");
    assert_eq!(repo.next(&["beta"]), "0.3.2-beta.1");

    // And again: the second squash counts from the second mark.
    repo.commit("feat(gui): another page (#17)");
    assert_eq!(repo.next(&["beta"]), "0.3.2-beta.2");
    repo.release("0.3.2");
    repo.commit("docs(repo): say more (#18)");

    assert_eq!(repo.next(&["beta"]), "0.3.3-beta.1");
}

#[test]
fn the_base_is_the_highest_stable_tag_even_one_head_does_not_hold() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#14)");
    // A feature branch started before the release, which tags commits it does not hold.
    repo.git(&["checkout", "--quiet", "-b", "feat/x"]);
    repo.commit("fix(cli): a typo");
    repo.git(&["checkout", "--quiet", "dev"]);
    repo.release("0.4.0");
    repo.git(&["checkout", "--quiet", "feat/x"]);

    assert_eq!(
        repo.next(&["alpha", "--branch", "feat/x"]),
        "0.4.1-alpha.feat-x.1"
    );
}

#[test]
fn an_unknown_channel_is_refused() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    let output = repo.version(&["nightly"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("stable, beta or alpha"),
        "{output:?}"
    );
}

#[test]
fn release_as_sets_the_next_version() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.1.1");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(tools): cut long results (#328)");
    repo.commit("feat(specs): the specs (#400)\n\nRelease-As: 0.2.0");
    repo.commit("fix(cli): a typo (#401)");

    assert_eq!(repo.next(&["stable"]), "0.2.0");
    // The betas follow it, and so do the alphas.
    assert_eq!(repo.next(&["beta"]), "0.2.0-beta.3");
    assert_eq!(
        repo.next(&["alpha", "--branch", "feat/x"]),
        "0.2.0-alpha.feat-x.3"
    );

    // Without it, the same commits give the patch.
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.1.1");
    repo.commit("feat(tools): cut long results (#328)");
    assert_eq!(repo.next(&["stable"]), "0.1.2");
}

#[test]
fn release_as_is_forgotten_once_released() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.1.1");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(specs): the specs (#400)\n\nRelease-As: 0.2.0");
    repo.release("0.2.0");
    repo.commit("feat(gui): add a page (#402)");

    assert_eq!(repo.next(&["stable"]), "0.2.1");
}

#[test]
fn release_as_is_read_on_dev_s_own_line_only() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.1.1");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.git(&["checkout", "--quiet", "-b", "feat/specs"]);
    repo.commit("feat(specs): a step\n\nRelease-As: 0.9.0");
    repo.git(&["checkout", "--quiet", "dev"]);
    repo.commit("fix(cli): a typo (#401)");
    repo.git(&[
        "merge",
        "--quiet",
        "--no-ff",
        "-m",
        "chore(repo): merge",
        "feat/specs",
    ]);

    assert_eq!(repo.next(&["stable"]), "0.1.2");
}

#[test]
fn release_as_below_the_computed_version_is_refused() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.1.1");
    repo.commit("feat(cli)!: rename the commands (#1)");
    repo.commit("fix(store): a lock (#2)\n\nRelease-As: 0.1.9");
    let refused = repo.head();

    for channel in ["stable", "beta", "alpha"] {
        let output = repo.version(&[channel, "--branch", "feat/x"]);
        assert_eq!(output.status.code(), Some(1), "{channel}: {output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(&refused), "{channel}: {error}");
        assert!(
            error.contains("0.1.9") && error.contains("0.2.0"),
            "{error}"
        );
    }
}

#[test]
fn a_malformed_release_as_is_refused() {
    for value in ["0.2", "v0.2.0", "0.2.0-beta.1", "soon", "0.2.0 now", ""] {
        let repo = Repo::new();
        repo.commit("feat(core): start");
        repo.tag("v0.1.1");
        repo.commit(&format!(
            "feat(specs): the specs (#400)\n\nRelease-As: {value}"
        ));
        let refused = repo.head();

        let output = repo.version(&["stable"]);

        assert_eq!(output.status.code(), Some(1), "{value:?}: {output:?}");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(&refused), "{value:?}: {error}");
        assert!(error.contains("Release-As"), "{value:?}: {error}");
    }
}

#[test]
fn a_release_as_equal_to_the_computed_version_changes_nothing() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.1.1");
    repo.commit("fix(store): a lock (#2)\n\nRelease-As: 0.1.2");

    assert_eq!(repo.next(&["stable"]), "0.1.2");
}

#[test]
fn a_fast_forward_release_after_the_two_squash_releases() {
    let repo = Repo::new();
    repo.released_by_squash_twice();
    repo.commit("feat(tools): cut long results (#350)");
    repo.commit("fix(store): a lock (#351)");
    // The record of 0.1.1 counts as one, as it did.
    assert_eq!(repo.next(&["beta"]), "0.1.2-beta.3");

    // The build of main, before the tag exists: the beta's version without its pre-release.
    let released = repo.head();
    repo.git(&["checkout", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--ff-only", "dev"]);
    assert_eq!(repo.next(&["stable", "--at", &released]), "0.1.2");
    assert_eq!(
        repo.changelog(&["--at", &released]),
        "### All changes\n\n#### Features\n\n- **tools**: cut long results (#350)\n\n\
         #### Fixes\n\n- **store**: a lock (#351)\n"
    );
    repo.tag("v0.1.2");
    repo.git(&["checkout", "--quiet", "dev"]);

    // Run again once tagged, it has nothing to release.
    let output = repo.version(&["stable", "--at", &released]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    // The next squash on dev is the first beta of the next version: no record counts.
    repo.commit("docs(repo): say more (#352)");
    assert_eq!(repo.next(&["beta"]), "0.1.3-beta.1");
    assert_eq!(
        repo.next(&["alpha", "--branch", "feat/x"]),
        "0.1.3-alpha.feat-x.1"
    );
    assert_eq!(
        repo.changelog(&[]),
        "### All changes\n\n#### Other changes\n\n- **repo**: say more (#352)\n"
    );
}

#[test]
fn a_second_fast_forward_release_counts_from_the_first() {
    let repo = Repo::new();
    repo.released_by_squash_twice();
    repo.commit("feat(tools): cut long results (#350)");
    repo.release_by_fast_forward("0.1.2");
    repo.commit("fix(cli): a flag (#360)");
    repo.commit("perf(store): fewer reads (#361)");
    let dev = repo.head();

    assert_eq!(repo.next(&["beta"]), "0.1.3-beta.2");
    let released = repo.release_by_fast_forward("0.1.3");

    assert_eq!(released, dev);
    assert_eq!(
        repo.changelog(&["v0.1.2..v0.1.3"]),
        "### All changes\n\n#### Fixes\n\n- **cli**: a flag (#360)\n\n\
         #### Performance\n\n- **store**: fewer reads (#361)\n"
    );
    repo.commit("feat(gui): another page (#362)");
    assert_eq!(repo.next(&["beta"]), "0.1.4-beta.1");
    assert_eq!(repo.next(&["stable"]), "0.1.4");
}

#[test]
fn release_as_across_a_fast_forward_release() {
    let repo = Repo::new();
    repo.released_by_squash_twice();
    repo.commit("feat(specs): the specs (#400)\n\nRelease-As: 0.2.0");
    repo.commit("fix(cli): a typo (#401)");
    assert_eq!(repo.next(&["beta"]), "0.2.0-beta.3");
    let dev = repo.head();
    assert_eq!(repo.next(&["stable", "--at", &dev]), "0.2.0");

    repo.release_by_fast_forward("0.2.0");
    repo.commit("feat(gui): add a page (#402)");

    // Forgotten once released: the next counts from the tag.
    assert_eq!(repo.next(&["beta"]), "0.2.1-beta.1");
    assert_eq!(repo.next(&["stable"]), "0.2.1");
}

// The maintenance lines.

/// main released 0.3.0, dev went on to the 0.4 betas, and the line `stable/0.3` started from
/// that tag with one of dev's fixes brought to it. On the line.
fn a_line_of_0_3() -> Repo {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#10)");
    repo.release_by_fast_forward("0.3.0");
    repo.commit("feat(cli)!: rename the commands (#11)");
    repo.commit("fix(store): a lock (#12)");
    repo.git(&["checkout", "--quiet", "-b", "stable/0.3", "v0.3.0"]);
    repo.commit("fix(store): a lock (#13)");
    repo
}

#[test]
fn a_line_counts_from_its_own_latest_tag() {
    let repo = a_line_of_0_3();
    repo.line_release("0.3.1");
    repo.commit("fix(cli): a typo (#14)");
    repo.commit("fix(gui): a colour (#15)");
    // dev's 0.4.0, higher, is not in the line's history: it counts for nothing there.
    repo.git(&["checkout", "--quiet", "dev"]);
    repo.release_by_fast_forward("0.4.0");
    repo.git(&["checkout", "--quiet", "stable/0.3"]);

    assert_eq!(repo.next(&["beta", "--branch", "stable/0.3"]), "0.3.2-rc.2");
    assert_eq!(repo.next(&["stable", "--branch", "stable/0.3"]), "0.3.2");
    // Its name as git has it when none is given.
    assert_eq!(repo.next(&["beta"]), "0.3.2-rc.2");
    assert!(
        repo.next(&["base", "--branch", "stable/0.3"])
            .starts_with("v0.3.1 "),
        "the line's own tag is its base"
    );
}

#[test]
fn a_line_bumps_the_patch_only() {
    let repo = a_line_of_0_3();
    repo.commit("feat(gui): a page after all (#14)");
    repo.commit("fix(cli)!: a flag renamed (#15)");
    repo.commit("fix(store): a new schema (#16)\n\nBREAKING CHANGE: the old store is not read");

    assert_eq!(repo.next(&["stable", "--branch", "stable/0.3"]), "0.3.1");
    assert_eq!(repo.next(&["beta", "--branch", "stable/0.3"]), "0.3.1-rc.4");

    // From 1.0 on too: a feature is the minor on dev, the patch on a line.
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v1.2.0");
    repo.git(&["checkout", "--quiet", "-b", "stable/1.2"]);
    repo.commit("feat(gui): a page after all (#14)");
    assert_eq!(repo.next(&["stable", "--branch", "stable/1.2"]), "1.2.1");
}

#[test]
fn release_as_is_refused_on_a_line() {
    let repo = a_line_of_0_3();
    repo.commit("fix(cli): a typo (#14)\n\nRelease-As: 0.3.5");
    let refused = repo.head();

    for channel in ["stable", "beta"] {
        let output = repo.version(&[channel, "--branch", "stable/0.3"]);
        assert_eq!(output.status.code(), Some(1), "{channel}: {output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(&refused), "{channel}: {error}");
        assert!(
            error.contains("Release-As") && error.contains("stable/0.3"),
            "{error}"
        );
    }
}

#[test]
fn dev_stays_above_a_version_a_line_released() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#10)");
    repo.release_by_fast_forward("0.3.0");
    repo.commit("fix(store): a lock (#12)");
    // Alone, dev's fix is the next patch.
    assert_eq!(repo.next(&["beta", "--branch", "dev"]), "0.3.1-beta.1");

    // The same fix released on the line as 0.3.1.
    repo.git(&["checkout", "--quiet", "-b", "stable/0.3", "v0.3.0"]);
    repo.commit("fix(store): a lock (#13)");
    repo.line_release("0.3.1");
    repo.git(&["checkout", "--quiet", "dev"]);

    // dev goes to the next minor, above every version of the line.
    assert_eq!(repo.next(&["beta", "--branch", "dev"]), "0.4.0-beta.1");
    let dev = repo.head();
    assert_eq!(
        repo.next(&["stable", "--branch", "main", "--at", &dev]),
        "0.4.0"
    );
    // And so does a branch of dev.
    repo.git(&["checkout", "--quiet", "-b", "fix/x"]);
    repo.commit("fix(cli): a typo");
    assert_eq!(
        repo.next(&["alpha", "--branch", "fix/x"]),
        "0.4.0-alpha.fix-x.2"
    );
}

#[test]
fn each_line_counts_its_own_with_stable_0_3_and_next_0_5_while_dev_is_0_4() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#10)");
    repo.release_by_fast_forward("0.3.0");
    repo.commit("feat(specs): the specs (#20)\n\nRelease-As: 0.4.0");
    // The fix line of 0.3, released once.
    repo.git(&["checkout", "--quiet", "-b", "stable/0.3", "v0.3.0"]);
    repo.commit("fix(gui): a colour (#14)");
    repo.line_release("0.3.1");
    repo.commit("fix(cli): a typo (#15)");
    // The version after dev's, started early, then dev merged forward into it.
    repo.git(&["checkout", "--quiet", "-b", "next/0.5", "dev"]);
    repo.commit("feat(remote): a phone (#30)");
    repo.git(&["checkout", "--quiet", "dev"]);
    repo.commit("fix(cli): a typo (#21)");
    repo.git(&["checkout", "--quiet", "next/0.5"]);
    repo.git(&[
        "merge",
        "--quiet",
        "--no-ff",
        "-m",
        "chore(repo): merge dev into next/0.5",
        "dev",
    ]);

    let on = |branch: &str| {
        repo.git(&["checkout", "--quiet", branch]);
        repo.next(&["beta", "--branch", branch])
    };
    assert_eq!(on("stable/0.3"), "0.3.2-rc.1");
    // dev above every stable line, below next.
    assert_eq!(on("dev"), "0.4.0-beta.2");
    assert_eq!(on("next/0.5"), "0.5.0-next.3");
    repo.git(&["checkout", "--quiet", "next/0.5"]);
    assert_eq!(repo.next(&["stable", "--branch", "next/0.5"]), "0.5.0");
}

#[test]
fn a_next_line_not_above_dev_is_refused() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(specs): the specs (#20)\n\nRelease-As: 0.4.0");
    repo.git(&["checkout", "--quiet", "-b", "next/0.4"]);
    repo.commit("feat(remote): a phone (#30)");

    let output = repo.version(&["beta", "--branch", "next/0.4"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("next/0.4 is not above dev's 0.4.0"),
        "{output:?}"
    );
}

#[test]
fn two_lines_at_once_each_count_their_own() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#10)");
    repo.release_by_fast_forward("0.3.0");
    repo.commit("feat(cli)!: rename the commands (#11)");
    repo.release_by_fast_forward("0.4.0");
    repo.commit("feat(store)!: a new schema (#12)");
    repo.commit("fix(gui): a colour (#13)");
    repo.git(&["checkout", "--quiet", "-b", "stable/0.3", "v0.3.0"]);
    repo.commit("fix(gui): a colour (#14)");
    repo.line_release("0.3.1");
    repo.commit("fix(cli): a typo (#15)");
    repo.git(&["checkout", "--quiet", "-b", "stable/0.4", "v0.4.0"]);
    repo.commit("fix(gui): a colour (#16)");

    let on = |line: &str, channel: &str| {
        repo.git(&["checkout", "--quiet", line]);
        repo.next(&[channel, "--branch", line])
    };
    assert_eq!(on("stable/0.3", "beta"), "0.3.2-rc.1");
    assert_eq!(on("stable/0.4", "beta"), "0.4.1-rc.1");
    assert_eq!(on("stable/0.4", "stable"), "0.4.1");

    repo.line_release("0.4.1");
    repo.commit("fix(cli): a typo (#17)");
    assert_eq!(on("stable/0.4", "beta"), "0.4.2-rc.1");
    assert_eq!(on("stable/0.3", "beta"), "0.3.2-rc.1");
    assert_eq!(on("dev", "beta"), "0.5.0-beta.2");
}

#[test]
fn dev_and_a_stable_line_never_compute_the_same_version() {
    // dev took only fixes since 0.3.0, and the line stable/0.3 one of them: both are 0.3.1
    // before either is released.
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#10)");
    repo.release_by_fast_forward("0.3.0");
    repo.commit("fix(store): a lock (#12)");
    repo.git(&["checkout", "--quiet", "-b", "stable/0.3", "v0.3.0"]);
    repo.commit("fix(store): a lock (#13)");

    let on = |branch: &str, channel: &str| {
        repo.git(&["checkout", "--quiet", branch]);
        repo.next(&[channel, "--branch", branch])
    };
    // Their pre-releases have names of their own.
    assert_eq!(on("dev", "beta"), "0.3.1-beta.1");
    assert_eq!(on("stable/0.3", "beta"), "0.3.1-rc.1");

    // dev released 0.3.1 first: the line goes above it, never to a tag dev holds.
    repo.git(&["checkout", "--quiet", "dev"]);
    repo.release_by_fast_forward("0.3.1");
    assert_eq!(on("stable/0.3", "beta"), "0.3.2-rc.1");
    assert_eq!(on("stable/0.3", "stable"), "0.3.2");
}

#[test]
fn a_next_line_publishes_next_pre_releases() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(specs): the specs (#20)\n\nRelease-As: 0.4.0");
    repo.git(&["checkout", "--quiet", "-b", "next/0.5"]);
    repo.commit("feat(remote): a phone (#30)");
    let sha = repo.head();

    assert_eq!(repo.next(&["beta", "--branch", "next/0.5"]), "0.5.0-next.3");
    repo.git(&["checkout", "--quiet", "dev"]);
    assert_eq!(repo.next(&["beta", "--branch", "dev"]), "0.4.0-beta.2");

    // CI publishes it under that name.
    let (output, outputs, _) = version_step(&repo, "next/0.5", "branch", &sha);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        outputs,
        "channel=next\nversion=0.5.0-next.3\ntag=v0.5.0-next.3\n"
    );
}

#[test]
fn an_rc_build_says_its_channel() {
    let repo = a_line_of_0_3();
    let sha = repo.head();
    repo.git(&["checkout", "--quiet", "dev"]);
    let (output, outputs, _) = version_step(&repo, "stable/0.3", "branch", &sha);
    assert!(output.status.success(), "{output:?}");
    assert!(outputs.starts_with("channel=rc\n"), "{outputs}");

    // The build is given that channel, and dev's stays the beta.
    let ci = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    assert!(
        ci.contains("FLOTTILLE_BUILD_CHANNEL: ${{ steps.version.outputs.channel }}"),
        "{ci}"
    );
    let (_, outputs, _) = version_step(&repo, "dev", "branch", &repo.head());
    assert!(outputs.starts_with("channel=beta\n"), "{outputs}");
}

#[test]
fn a_stable_line_publishes_rc_pre_releases() {
    let repo = a_line_of_0_3();
    let sha = repo.head();
    assert_eq!(repo.next(&["beta", "--branch", "stable/0.3"]), "0.3.1-rc.1");
    repo.commit("fix(cli): a typo (#14)");
    assert_eq!(repo.next(&["beta", "--branch", "stable/0.3"]), "0.3.1-rc.2");
    repo.git(&["checkout", "--quiet", "dev"]);

    let (output, outputs, _) = version_step(&repo, "stable/0.3", "branch", &sha);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(outputs, "channel=rc\nversion=0.3.1-rc.1\ntag=v0.3.1-rc.1\n");

    // A line's release takes its rc, nothing else of that version.
    repo.git(&["tag", "v0.3.1-beta.1", &sha]);
    let released = run_release_of(&repo, "stable/0.3", &sha, "refs/heads/dev");
    assert!(!released.output.status.success(), "{:?}", released.output);
    assert!(repo.git(&["tag", "--list", "v0.3.1"]).is_empty());
    repo.git(&["tag", "v0.3.1-rc.1", &sha]);
    let released = run_release_of(&repo, "stable/0.3", &sha, "refs/heads/dev");
    assert!(released.output.status.success(), "{:?}", released.output);
    assert_eq!(repo.git(&["rev-parse", "v0.3.1^{commit}"]), sha);
}

#[test]
fn a_line_with_nothing_since_its_tag_publishes_nothing() {
    // Just cut from its tag: no commit of its own.
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#10)");
    repo.release_by_fast_forward("0.3.0");
    repo.commit("fix(store): a lock (#12)");
    repo.git(&["checkout", "--quiet", "-b", "stable/0.3", "v0.3.0"]);
    let cut = repo.head();
    let nothing = |repo: &Repo| {
        for channel in ["stable", "beta", "alpha"] {
            let output = repo.version(&[channel, "--branch", "stable/0.3"]);
            assert_eq!(output.status.code(), Some(3), "{channel}: {output:?}");
            assert_eq!(String::from_utf8_lossy(&output.stdout), "", "{channel}");
        }
    };
    nothing(&repo);

    // Nor right after its release.
    repo.commit("fix(store): a lock (#13)");
    repo.line_release("0.3.1");
    nothing(&repo);
    repo.git(&["checkout", "--quiet", "dev"]);

    // CI's push run of the line skips.
    let (output, outputs, _) = version_step(&repo, "stable/0.3", "branch", &cut);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(outputs, "skip=true\n");
}

// The changelog.

#[test]
fn the_changelog_groups_the_commits_by_type_with_their_pull_requests() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("fix(cli): a typo (#13)");
    repo.commit("feat(gui): add a page (#14)");
    repo.commit("docs(repo): say more (#15)");
    repo.commit("feat(store)!: a new schema (#16)");
    repo.commit("perf(store): fewer reads (#17)");
    repo.commit("Merge branch 'dev'");

    // No pull request says a note: the raw list is all there is, as it always was.
    assert_eq!(
        repo.changelog(&[]),
        "### All changes\n\n\
         #### Breaking changes\n\n- **store**: a new schema (#16)\n\n\
         #### Features\n\n- **gui**: add a page (#14)\n- **store**: a new schema (#16)\n\n\
         #### Fixes\n\n- **cli**: a typo (#13)\n\n\
         #### Performance\n\n- **store**: fewer reads (#17)\n\n\
         #### Other changes\n\n- **repo**: say more (#15)\n"
    );
}

#[test]
fn the_changelog_of_a_stable_release_lists_the_dev_commits_between_the_two_marks() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("fix(cli): a typo (#13)");
    repo.release("0.3.1");
    repo.commit("feat(gui): add a page (#14)");
    repo.commit("docs(repo): say more (#15)");

    let dev = repo.squash_dev_into_main();

    assert_eq!(
        repo.changelog(&["--at", &dev]),
        "### All changes\n\n\
         #### Features\n\n- **gui**: add a page (#14)\n\n\
         #### Other changes\n\n- **repo**: say more (#15)\n"
    );
}

#[test]
fn two_releases_in_a_row_the_second_merges_its_version_is_the_next_and_its_changelog_is_dev_s() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#14)");
    // The squash's title is the release pull request's, an Angular one.
    let dev = repo.head();
    repo.git(&["checkout", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--squash", "dev"]);
    repo.git(&[
        "commit",
        "--quiet",
        "-m",
        "chore(repo): release 0.4.0 (#20)",
    ]);
    repo.tag("v0.4.0");
    repo.git(&["tag", "released/v0.4.0", &dev]);
    repo.git(&["checkout", "--quiet", "dev"]);
    repo.record_release_in_dev("0.4.0");
    // Both sides change the same file: without the record, the base is the first commit.
    repo.commit("fix(cli): a typo (#21)");
    std::fs::write(repo.dir.path().join("file-1"), "changed again").unwrap();
    repo.git(&[
        "commit",
        "--quiet",
        "--all",
        "-m",
        "docs(repo): say more (#22)",
    ]);
    let dev = repo.head();

    // The second pull request dev → main merges without a conflict.
    let merged = repo
        .git_command(&["merge-tree", "--write-tree", "main", "dev"])
        .output()
        .unwrap();
    assert!(merged.status.success(), "{merged:?}");
    assert_eq!(repo.next(&["stable", "--at", &dev]), "0.4.1");
    assert_eq!(
        repo.changelog(&["--at", &dev]),
        "### All changes\n\n\
         #### Fixes\n\n- **cli**: a typo (#21)\n\n\
         #### Other changes\n\n- **repo**: say more (#22)\n"
    );
}

#[test]
fn a_line_s_changelog_lists_its_commits_since_its_previous_tag() {
    let repo = a_line_of_0_3();
    repo.line_release("0.3.1");
    repo.commit("fix(cli): a typo (#14)");
    repo.git(&["checkout", "--quiet", "dev"]);
    repo.release_by_fast_forward("0.4.0");
    repo.git(&["checkout", "--quiet", "stable/0.3"]);

    assert_eq!(
        repo.changelog(&["--at", "HEAD", "--branch", "stable/0.3"]),
        "### All changes\n\n#### Fixes\n\n- **cli**: a typo (#14)\n"
    );
}

// The notes of a release (#1178).

#[test]
fn a_note_line_becomes_the_entry() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("feat(gui): a page for the changes (#14)");
    repo.pr(
        14,
        "I add a page for the changes.\r\n\r\nNote: Read what changed on a page of its own.\r\n\r\nTests: group_window_shell\r\n\r\nCloses #3",
    );

    // The sentence, the number beside it; the subject stays in the raw list.
    assert_eq!(
        repo.changelog(&[]),
        "### Window\n\n- Read what changed on a page of its own. (#14)\n\n\
         ### All changes\n\n#### Features\n\n- **gui**: a page for the changes (#14)\n"
    );
}

#[test]
fn a_pull_request_without_a_note_is_only_in_the_raw_list() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("refactor(store): split a module (#21)");
    repo.commit("feat(gui): a page for the changes (#22)");
    repo.commit("fix(cli): a typo (#23)");
    repo.commit("docs(repo): say more");
    repo.pr(21, "I split a module.\n\nCloses #5");
    repo.pr(22, "Note: Read what changed on a page of its own.");
    repo.pr(23, "I fix a typo.");

    assert_eq!(
        repo.changelog(&[]),
        "### Window\n\n- Read what changed on a page of its own. (#22)\n\n\
         ### All changes\n\n\
         #### Features\n\n- **gui**: a page for the changes (#22)\n\n\
         #### Fixes\n\n- **cli**: a typo (#23)\n\n\
         #### Other changes\n\n- **store**: split a module (#21)\n- **repo**: say more\n"
    );
}

#[test]
fn a_note_from_an_outside_author_is_not_used() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("feat(gui): a page for the changes (#14)");
    repo.commit("feat(gui): a second page (#15)");
    // The description is read at release time, after the merge: whoever wrote the pull request
    // can have changed it since. Only a maintainer's note is the release's.
    repo.pr(14, "Note: Read what changed on a page of its own.");
    repo.pr_by(14, "CONTRIBUTOR");
    repo.pr(15, "Note: See the second page.");
    repo.pr_by(15, "COLLABORATOR");

    assert_eq!(
        repo.changelog(&[]),
        "### Window\n\n- See the second page. (#15)\n\n\
         ### All changes\n\n#### Features\n\n\
         - **gui**: a page for the changes (#14)\n- **gui**: a second page (#15)\n"
    );
}

#[test]
fn a_pull_request_that_cannot_be_read_fails_the_changelog() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("fix(cli): a typo (#23)");
    repo.pr_unreadable(23);

    // A release built without its note cannot be edited afterwards: the build stops instead.
    let output = repo
        .with_gh(
            Command::new(tool("tools/changelog.sh"))
                .current_dir(repo.dir.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1"),
        )
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("#23"),
        "{output:?}"
    );
}

#[test]
fn notes_are_grouped_by_area_with_fixes_last() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    let commits = [
        ("fix(gui): a link opens (#31)", "A link opens again."),
        (
            "feat(daemon): a quiet start (#32)",
            "The daemon starts quietly.",
        ),
        (
            "feat(hub): sign every action (#33)",
            "Drive another machine, each action signed.",
        ),
        (
            "feat(gui): a mark at the foot (#34)",
            "See what is new from the sidebar.",
        ),
        (
            "feat(sessions): resume a session (#35)",
            "Resume a session where it stopped.",
        ),
        (
            "feat(mcp): list the tools (#36)",
            "Ask the tools an agent has.",
        ),
        (
            "feat(git): follow a branch (#37)",
            "Follow a branch from its project.",
        ),
        (
            "feat(cli): remote on and off (#38)",
            "Turn remote on from the command line.",
        ),
        (
            "fix(store): a lock (#39)",
            "A project can be removed while it is busy.",
        ),
        (
            "feat(android): open a session (#40)",
            "Open a session from the phone.",
        ),
        (
            "perf(acp): fewer reads (#41)",
            "Long conversations open faster.",
        ),
    ];
    for (subject, note) in commits {
        repo.commit(subject);
        let number = subject.rsplit_once("(#").unwrap().1.trim_end_matches(')');
        repo.pr(
            number.parse().unwrap(),
            &format!("Why.\n\nNote: {note}\n\nCloses #1"),
        );
    }

    let changelog = repo.changelog(&[]);

    // The areas in their order, the notes of each oldest first, the fixes last, then the raw list.
    let notes = changelog.split("### All changes").next().unwrap();
    assert_eq!(
        notes,
        "### Remote\n\n\
         - Drive another machine, each action signed. (#33)\n\
         - Turn remote on from the command line. (#38)\n\
         - Open a session from the phone. (#40)\n\n\
         ### Window\n\n- See what is new from the sidebar. (#34)\n\n\
         ### Agents\n\n- Resume a session where it stopped. (#35)\n\
         - Long conversations open faster. (#41)\n\n\
         ### Tools\n\n- Ask the tools an agent has. (#36)\n\n\
         ### Projects\n\n- Follow a branch from its project. (#37)\n\n\
         ### Other\n\n- The daemon starts quietly. (#32)\n\n\
         ### Fixed\n\n- A link opens again. (#31)\n- A project can be removed while it is busy. (#39)\n\n"
    );
    assert!(
        changelog.contains("### All changes\n\n#### Features\n\n"),
        "{changelog}"
    );
}

#[test]
fn the_token_is_given_to_the_step_that_reads_the_notes_only() {
    let workflow = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    // The build job: from its environment (the head's commit and branch) to its first step's run.
    let build = workflow
        .split("      HEAD_SHA: ${{ github.event.pull_request.head.sha }}\n    outputs:")
        .nth(1)
        .expect("the build job");
    let (job, steps) = build.split_once("    steps:").unwrap();
    // The job's own environment reaches every step, the build scripts and the dependencies.
    assert!(!job.contains("GH_TOKEN"), "{job}");
    let (version, _) = steps
        .split_once("name: The channel and the version")
        .unwrap()
        .1
        .split_once("        run: |")
        .unwrap();
    assert!(
        version.contains("GH_TOKEN: ${{ github.token }}"),
        "{version}"
    );
}

// The shape check asks for a note (#1178).

/// What `.github/scripts/pr-note.sh` says of a pull request titled `title` that changes `changed`
/// (one path per line), whose description is `body`: its workflow commands, on stdout.
fn note_check(title: &str, changed: &[&str], body: &str) -> String {
    let output = Command::new("bash")
        .arg(tool(".github/scripts/pr-note.sh"))
        .env("TITLE", title)
        .env("BODY", body)
        .env("CHANGED", changed.join("\n"))
        .output()
        .unwrap();
    // A warning, never a failure: the pull request stays mergeable.
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn the_shape_check_warns_on_a_feature_without_a_note() {
    let product = ["crates/flottille/src/daemon.rs", "docs/design.md"];
    for title in ["feat(gui): a page", "fix(cli): a typo"] {
        let said = note_check(title, &product, "I do it.\n\nCloses #1");
        assert!(said.starts_with("::warning"), "{title}: {said}");
        assert!(said.contains("Note:"), "{said}");
    }
    // A note, said at the start of its line, satisfies it.
    assert_eq!(
        note_check(
            "feat(gui): a page",
            &product,
            "I do it.\n\nNote: Read what changed.\n\nCloses #1"
        ),
        ""
    );
    // Nothing the user can see: the type, or what the pull request touches, says so.
    for title in [
        "refactor(core): tidy",
        "docs(repo): say more",
        "ci(ci): faster",
        "test(gui): more",
    ] {
        assert_eq!(note_check(title, &product, "Closes #1"), "", "{title}");
    }
    assert_eq!(
        note_check(
            "feat(repo): a tool",
            &[
                "tools/changelog.sh",
                "crates/flottille-fake-gh/src/main.rs",
                "crates/flottille/tests/a.rs"
            ],
            "Closes #1"
        ),
        ""
    );
    // The title is the author's: it is never said inside a workflow command, where a newline or
    // `%0A` in it would start another one.
    let said = note_check(
        "feat(gui): a page%0A::error::injected\r",
        &product,
        "Closes #1",
    );
    assert!(said.starts_with("::warning"), "{said}");
    assert!(!said.contains("injected"), "{said}");
    // The workflow runs it, and its step cannot fail the check.
    let workflow = std::fs::read_to_string(tool(".github/workflows/pull-request.yml")).unwrap();
    assert!(
        workflow.contains(".github/scripts/pr-note.sh"),
        "{workflow}"
    );
}

// The archive.

/// `tools/package.sh <version> <binary> <folder>`: the archive and its sum, in `folder`.
pub(crate) fn package(version: &str, binary: &Path, out: &Path) -> Output {
    Command::new(tool("tools/package.sh"))
        .args([version, binary.to_str().unwrap(), out.to_str().unwrap()])
        .output()
        .unwrap()
}

/// The SHA-256 of a file. It is read in chunks: the `flottille` binary weighs over a gigabyte in
/// a debug build and the tests run side by side, so none of them may hold it whole.
pub(crate) fn sha256(path: impl AsRef<Path>) -> String {
    let mut file = std::fs::File::open(path).unwrap();
    let mut hasher = Sha256::new();
    let mut chunk = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut chunk).unwrap();
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn a_changed_byte_changes_the_sum_of_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    // Longer than a chunk, the change beyond the first one: a sum that stopped early would miss it.
    let mut bytes = vec![7u8; 5 << 20];
    std::fs::write(&path, &bytes).unwrap();
    let before = sha256(&path);
    assert_eq!(sha256(&path), before);

    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&path, &bytes).unwrap();

    assert_ne!(sha256(&path), before);
}

/// The most memory the process has held at once, in bytes (`VmHWM`).
fn peak_memory() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap();
    kib << 10
}

#[test]
fn the_sum_of_a_file_does_not_hold_it_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big");
    // Sparse: it costs the disk nothing, and it is read as zeros.
    let big = 192 << 20;
    std::fs::File::create(&path).unwrap().set_len(big).unwrap();
    let before = peak_memory();

    sha256(&path);

    // Other tests run in this process meanwhile: a margin, far below the file's size.
    let held = peak_memory() - before;
    assert!(held < big / 4, "the sum held {} MiB", held >> 20);
}

#[test]
fn the_archive_holds_the_binary_both_licences_and_the_desktop_files_with_its_sum_beside_it() {
    let out = tempfile::tempdir().unwrap();
    let programs = tempfile::tempdir().unwrap();
    let binary = standing_for(programs.path(), "0.4.0-beta.2");

    let output = package("0.4.0-beta.2", &binary, out.path());

    assert!(output.status.success(), "{output:?}");
    let archive = out
        .path()
        .join("flottille-0.4.0-beta.2-x86_64-linux.tar.gz");
    // The archive only moves bytes: the real binary (a gigabyte in a debug build) would fill a
    // small /tmp for nothing.
    assert!(std::fs::metadata(&archive).unwrap().len() < 1 << 20);
    let sum = std::fs::read_to_string(archive.with_extension("gz.sha256")).unwrap();
    assert_eq!(
        sum,
        format!(
            "{}  flottille-0.4.0-beta.2-x86_64-linux.tar.gz\n",
            sha256(&archive)
        )
    );
    let listed = Command::new("tar")
        .args(["-tzf", archive.to_str().unwrap()])
        .output()
        .unwrap();
    let mut names: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter(|name| !name.ends_with('/'))
        .map(str::to_owned)
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "flottille-0.4.0-beta.2/LICENSE",
            "flottille-0.4.0-beta.2/LICENSE-fonts.txt",
            "flottille-0.4.0-beta.2/flottille",
            "flottille-0.4.0-beta.2/share/applications/flottille.desktop",
            "flottille-0.4.0-beta.2/share/icons/hicolor/128x128/apps/flottille.png",
            "flottille-0.4.0-beta.2/share/icons/hicolor/256x256/apps/flottille.png",
            "flottille-0.4.0-beta.2/share/icons/hicolor/48x48/apps/flottille.png",
            "flottille-0.4.0-beta.2/share/icons/hicolor/scalable/apps/flottille.svg",
        ]
    );
}

// The releases, served locally.

/// A local server standing for GitHub: the releases' API under `/repos/o/r/` and their assets
/// under `/download/<tag>/`. Each request's path is noted.
pub(crate) struct Releases {
    base: String,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    pub(crate) asked: Arc<Mutex<Vec<String>>>,
    held: Held,
    /// Each release made is published a second after the previous one.
    published: AtomicUsize,
}

/// The paths whose download stops halfway, each until its [`Hold`] is let go.
type Held = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

/// A download held halfway (`Releases::hold`): the server sends half of the body, then waits for
/// this to be released, or dropped.
pub(crate) struct Hold(Arc<AtomicBool>);

impl Hold {
    pub(crate) fn release(self) {}
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// How long a download held waits for its release at most: a test that forgot it does not hang.
const HELD_AT_MOST: Duration = Duration::from_secs(60);

fn answer(
    mut stream: TcpStream,
    served: &Mutex<HashMap<String, Vec<u8>>>,
    noted: &Mutex<Vec<String>>,
    held: &Mutex<HashMap<String, Arc<AtomicBool>>>,
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
    }
    let head = line.split_whitespace().next() == Some("HEAD");
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
    noted.lock().unwrap().push(path.clone());
    let body = served.lock().unwrap().get(&path).cloned();
    let hold = held.lock().unwrap().get(&path).cloned();
    let _ = match body {
        Some(body) => write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .and_then(|()| match (hold, head) {
            (_, true) => Ok(()),
            (Some(hold), false) => {
                let (first, rest) = body.split_at(body.len() / 2);
                stream.write_all(first)?;
                stream.flush()?;
                let until = Instant::now() + HELD_AT_MOST;
                while !hold.load(Ordering::SeqCst) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(20));
                }
                stream.write_all(rest)
            }
            (None, false) => stream.write_all(&body),
        }),
        None => write!(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
    };
}

impl Releases {
    /// The download at `path` stops halfway until the hold returned is released or dropped.
    pub(crate) fn hold(&self, path: &str) -> Hold {
        let flag = Arc::new(AtomicBool::new(false));
        self.held
            .lock()
            .unwrap()
            .insert(path.to_owned(), flag.clone());
        Hold(flag)
    }

    pub(crate) fn serve() -> Releases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let held: Held = Arc::default();
        let (served, noted, waiting) = (files.clone(), asked.clone(), held.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (served, noted, waiting) = (served.clone(), noted.clone(), waiting.clone());
                // One thread for each request: a download held halfway blocks only itself.
                std::thread::spawn(move || answer(stream, &served, &noted, &waiting));
            }
        });
        Releases {
            base,
            files,
            asked,
            held,
            published: AtomicUsize::new(0),
        }
    }

    /// The API's base, as `FLOTTILLE_RELEASES` takes it.
    pub(crate) fn api(&self) -> String {
        format!("{}/repos/o/r", self.base)
    }

    pub(crate) fn put(&self, path: &str, body: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_owned(), body);
    }

    /// The list the API answers in GitHub's own layout, not `list`'s short one: `author` before
    /// the tag, `published_at` (`null` on a draft) before the assets, an `uploader` object in each
    /// asset, and a body of commas and escaped quotes at the end.
    pub(crate) fn list_as_github(&self, releases: &[&serde_json::Value]) {
        let each: Vec<String> = releases
            .iter()
            .map(|r| {
                let assets: Vec<String> = r["assets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|a| {
                        format!(
                            r#"{{"name":{},"uploader":{{"login":"kris","id":1,"type":"User"}},"browser_download_url":{}}}"#,
                            a["name"], a["browser_download_url"]
                        )
                    })
                    .collect();
                format!(
                    r#"{{"url":"https://api.github.com/repos/o/r/releases/1","author":{{"login":"kris","id":1,"type":"User"}},"tag_name":{},"draft":{},"prerelease":{},"published_at":{},"assets":[{}],"body":"Changelog, with \"quotes\", and commas.\n- **cli**: one, two"}}"#,
                    r["tag_name"],
                    r["published_at"].is_null(),
                    r["prerelease"],
                    r["published_at"],
                    assets.join(",")
                )
            })
            .collect();
        self.put(
            "/repos/o/r/releases?per_page=100",
            format!("[{}]", each.join(",")).into_bytes(),
        );
    }

    /// The list of releases the API gives, with each one's keys in GitHub's order (the tag, then
    /// when it was published, then its assets), which `install.sh` reads without a JSON parser.
    pub(crate) fn list(&self, releases: &[&serde_json::Value]) {
        let each: Vec<String> = releases
            .iter()
            .map(|r| {
                format!(
                    r#"{{"tag_name":{},"prerelease":{},"published_at":{},"assets":{}}}"#,
                    r["tag_name"], r["prerelease"], r["published_at"], r["assets"]
                )
            })
            .collect();
        self.put(
            "/repos/o/r/releases?per_page=100",
            format!("[{}]", each.join(",")).into_bytes(),
        );
    }

    /// A release of that tag, holding the archive of `binary` at `version` and its sum (`sum`
    /// in place of the right one when given). Its API entry, which is also what the tag's own
    /// address answers.
    pub(crate) fn release(
        &self,
        tag: &str,
        version: &str,
        binary: &Path,
        sum: Option<&str>,
    ) -> serde_json::Value {
        let out = tempfile::tempdir().unwrap();
        let output = package(version, binary, out.path());
        assert!(output.status.success(), "{output:?}");
        let name = format!("flottille-{version}-x86_64-linux.tar.gz");
        let archive = std::fs::read(out.path().join(&name)).unwrap();
        let sum = match sum {
            Some(sum) => format!("{sum}  {name}\n"),
            None => std::fs::read_to_string(out.path().join(format!("{name}.sha256"))).unwrap(),
        };
        self.put(&format!("/download/{tag}/{name}"), archive);
        self.put(&format!("/download/{tag}/{name}.sha256"), sum.into_bytes());
        let url = |name: &str| format!("{}/download/{tag}/{name}", self.base);
        let n = self.published.fetch_add(1, Ordering::SeqCst);
        let release = json!({
            "tag_name": tag,
            "prerelease": tag.contains('-'),
            "published_at": format!("2026-11-04T09:{:02}:{:02}Z", n / 60, n % 60),
            "assets": [
                {"name": name, "browser_download_url": url(&name)},
                {"name": format!("{name}.sha256"), "browser_download_url": url(&format!("{name}.sha256"))},
            ],
        });
        self.put(
            &format!("/repos/o/r/releases/tags/{tag}"),
            release.to_string().into_bytes(),
        );
        release
    }
}

/// A program that says the version it stands for, as `flottille --version` does.
pub(crate) fn standing_for(dir: &Path, version: &str) -> PathBuf {
    let path = dir.join(format!("flottille-{version}"));
    std::fs::write(
        &path,
        format!("#!/bin/sh\necho \"flottille {version} (test, 000000000000)\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    path
}

/// Three channels: a stable release, a beta (the newest of the list) and a branch's alpha.
fn three_channels(releases: &Releases, programs: &Path) {
    let stable = releases.release("v0.4.0", "0.4.0", &standing_for(programs, "0.4.0"), None);
    let beta = releases.release(
        "v0.4.1-beta.3",
        "0.4.1-beta.3",
        &standing_for(programs, "0.4.1-beta.3"),
        None,
    );
    let alpha = releases.release(
        "v0.5.0-alpha.feat-46-packages.7",
        "0.5.0-alpha.feat-46-packages.7",
        &standing_for(programs, "0.5.0-alpha.feat-46-packages.7"),
        None,
    );
    releases.put(
        "/repos/o/r/releases/latest",
        stable.to_string().into_bytes(),
    );
    releases.list(&[&alpha, &beta, &stable]);
}

/// `install.sh` with these arguments, for a throwaway home.
fn install(home: &Path, releases: &Releases, args: &[&str]) -> Output {
    Command::new("sh")
        .arg(tool("install.sh"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", home)
        .env("FLOTTILLE_RELEASES", releases.api())
        .output()
        .unwrap()
}

#[test]
fn install_sh_in_a_throwaway_home_installs_each_channel_and_flot_answers() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());

    for (args, version) in [
        (vec![], "0.4.0"),
        (vec!["--channel", "stable"], "0.4.0"),
        (vec!["--channel", "beta"], "0.4.1-beta.3"),
        (
            vec!["--channel", "alpha", "--branch", "feat/46-packages"],
            "0.5.0-alpha.feat-46-packages.7",
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let output = install(home.path(), &releases, &args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        let bin = home.path().join(".local/bin");
        assert!(
            std::fs::symlink_metadata(bin.join("flot"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let said = Command::new(bin.join("flot"))
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&said.stdout).trim(),
            format!("flottille {version} (test, 000000000000)"),
            "{args:?}"
        );
    }
}

#[test]
fn install_sh_refuses_a_wrong_sum_and_installs_nothing() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let stable = releases.release(
        "v0.4.0",
        "0.4.0",
        &standing_for(programs.path(), "0.4.0"),
        Some(&"0".repeat(64)),
    );
    releases.put(
        "/repos/o/r/releases/latest",
        stable.to_string().into_bytes(),
    );
    let home = tempfile::tempdir().unwrap();

    let output = install(home.path(), &releases, &[]);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("SHA-256"),
        "{output:?}"
    );
    assert!(!home.path().join(".local/bin/flottille").exists());
    assert!(!home.path().join(".local/bin/flot").exists());
}

#[test]
fn install_sh_says_when_a_branch_has_no_alpha() {
    let releases = Releases::serve();
    let home = tempfile::tempdir().unwrap();

    let output = install(
        home.path(),
        &releases,
        &["--channel", "alpha", "--branch", "feat/none"],
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no alpha release for feat/none"),
        "{output:?}"
    );
}

/// Betas as (version, when published), listed in that order, as the API lists them: not
/// necessarily the order they were published in.
fn betas(releases: &Releases, programs: &Path, published: &[(&str, &str)]) {
    let made: Vec<serde_json::Value> = published
        .iter()
        .map(|(version, at)| {
            let mut release = releases.release(
                &format!("v{version}"),
                version,
                &standing_for(programs, version),
                None,
            );
            release["published_at"] = json!(at);
            release
        })
        .collect();
    releases.list(&made.iter().collect::<Vec<_>>());
}

/// The case that made #413: a build of `dev` numbered 0.2.0 before the numbering changed
/// (#358), published before the later 0.1.2 builds, and listed first.
const STRAY: [(&str, &str); 3] = [
    ("0.2.0-beta.5", "2026-11-04T09:34:08Z"),
    ("0.1.2-beta.15", "2026-11-04T10:42:54Z"),
    ("0.1.2-beta.14", "2026-11-04T10:20:05Z"),
];

/// The version a program installed in `home` says it stands for.
fn installed_version(home: &Path) -> String {
    let said = Command::new(home.join(".local/bin/flottille"))
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8_lossy(&said.stdout).trim().to_owned()
}

#[test]
fn install_sh_takes_the_most_recent_build_and_an_exact_version() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    betas(&releases, programs.path(), &STRAY);

    // The latest published of the beta channel, not the highest version.
    let home = tempfile::tempdir().unwrap();
    let output = install(home.path(), &releases, &["--channel", "beta"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        installed_version(home.path()),
        "flottille 0.1.2-beta.15 (test, 000000000000)"
    );

    // An exact version, of any channel.
    for version in ["0.2.0-beta.5", "0.4.0", "0.5.0-alpha.feat-46-packages.7"] {
        let home = tempfile::tempdir().unwrap();
        let output = install(home.path(), &releases, &["--version", version]);
        assert!(output.status.success(), "{version}: {output:?}");
        assert_eq!(
            installed_version(home.path()),
            format!("flottille {version} (test, 000000000000)")
        );
    }

    // One that does not exist: refused, with the close ones, and nothing installed.
    let home = tempfile::tempdir().unwrap();
    let output = install(home.path(), &releases, &["--version", "0.1.2-beta.16"]);
    assert!(!output.status.success(), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("no release 0.1.2-beta.16"), "{said}");
    assert!(said.contains("0.1.2-beta.15"), "{said}");
    assert!(!home.path().join(".local/bin/flottille").exists());

    // A version is not a channel.
    let output = install(
        home.path(),
        &releases,
        &["--version", "0.4.0", "--channel", "beta"],
    );
    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn the_beta_channel_never_takes_a_next_or_rc_release() {
    // A stable line's rc and the next version's pre-release, published after dev's beta.
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    betas(
        &releases,
        programs.path(),
        &[
            ("0.5.0-next.2", "2026-11-04T11:00:00Z"),
            ("0.3.2-rc.1", "2026-11-04T10:00:00Z"),
            ("0.4.0-beta.3", "2026-11-04T09:00:00Z"),
        ],
    );

    let home = tempfile::tempdir().unwrap();
    let output = install(home.path(), &releases, &["--channel", "beta"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        installed_version(home.path()),
        "flottille 0.4.0-beta.3 (test, 000000000000)"
    );
    // Each is installed by its version.
    for version in ["0.5.0-next.2", "0.3.2-rc.1"] {
        let home = tempfile::tempdir().unwrap();
        let output = install(home.path(), &releases, &["--version", version]);
        assert!(output.status.success(), "{version}: {output:?}");
        assert_eq!(
            installed_version(home.path()),
            format!("flottille {version} (test, 000000000000)")
        );
    }

    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.4.0-beta.3 (test, 000000000000)"
    );
}

#[test]
fn install_sh_does_not_offer_a_draft() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let published = releases.release(
        "v0.4.1-beta.3",
        "0.4.1-beta.3",
        &standing_for(programs.path(), "0.4.1-beta.3"),
        None,
    );
    let mut draft = releases.release(
        "v0.4.1-beta.4",
        "0.4.1-beta.4",
        &standing_for(programs.path(), "0.4.1-beta.4"),
        None,
    );
    draft["published_at"] = json!(null);
    // The draft is listed after the release published last: it takes no date of that one.
    releases.list_as_github(&[&published, &draft]);

    let home = tempfile::tempdir().unwrap();
    let output = install(home.path(), &releases, &["--channel", "beta"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        installed_version(home.path()),
        "flottille 0.4.1-beta.3 (test, 000000000000)"
    );

    // With no release published, the draft alone is not offered.
    releases.list_as_github(&[&draft]);
    let home = tempfile::tempdir().unwrap();
    let output = install(home.path(), &releases, &["--channel", "beta"]);
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("there is no beta release"),
        "{output:?}"
    );
    assert!(!home.path().join(".local/bin/flottille").exists());
}

#[test]
fn install_sh_says_a_network_failure_is_not_a_missing_release() {
    // Nothing listens on that port.
    let api = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/repos/o/r", listener.local_addr().unwrap())
    };
    for args in [
        vec![],
        vec!["--channel", "beta"],
        vec!["--channel", "alpha", "--branch", "feat/x"],
    ] {
        let home = tempfile::tempdir().unwrap();
        let output = Command::new("sh")
            .arg(tool("install.sh"))
            .args(&args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", home.path())
            .env("FLOTTILLE_RELEASES", &api)
            .output()
            .unwrap();

        assert!(!output.status.success(), "{args:?}: {output:?}");
        let said = String::from_utf8_lossy(&output.stderr);
        assert!(said.contains("cannot read"), "{args:?}: {said}");
        assert!(!said.contains("there is no"), "{args:?}: {said}");
    }
}

#[test]
fn install_sh_refuses_a_channel_without_a_value() {
    let releases = Releases::serve();
    for args in [vec!["--channel"], vec!["--channel", "alpha", "--branch"]] {
        let home = tempfile::tempdir().unwrap();

        let output = install(home.path(), &releases, &args);

        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("needs a value"),
            "{args:?}: {output:?}"
        );
    }
}

#[test]
fn install_sh_installs_even_when_the_new_binary_does_not_answer_its_version() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let mute = programs.path().join("mute");
    std::fs::write(&mute, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(&mute, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let stable = releases.release("v0.4.0", "0.4.0", &mute, None);
    releases.put(
        "/repos/o/r/releases/latest",
        stable.to_string().into_bytes(),
    );
    let home = tempfile::tempdir().unwrap();

    let output = install(home.path(), &releases, &[]);

    assert!(output.status.success(), "{output:?}");
    assert!(home.path().join(".local/bin/flottille").exists());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Installed 0.4.0"),
        "{output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--version did not answer"),
        "{output:?}"
    );
}

#[test]
fn install_sh_cut_short_by_the_network_runs_nothing() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let script = std::fs::read_to_string(tool("install.sh")).unwrap();
    let lines: Vec<&str> = script.split_inclusive('\n').collect();

    // `curl … | sh` that got only the first lines of the script.
    for cut in 0..lines.len() {
        let home = tempfile::tempdir().unwrap();
        let mut sh = Command::new("sh")
            .arg("-s")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", home.path())
            .env("FLOTTILLE_RELEASES", releases.api())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        sh.stdin
            .take()
            .unwrap()
            .write_all(lines[..cut].concat().as_bytes())
            .unwrap();
        let output = sh.wait_with_output().unwrap();

        assert!(
            !home.path().join(".local").exists(),
            "cut after {cut} lines: {output:?}"
        );
    }
    assert!(
        releases
            .asked
            .lock()
            .unwrap()
            .iter()
            .all(|p| !p.contains("/download/")),
        "a cut script downloaded something"
    );
}

/// The alphas of two branches, each its own release, listed newest first as the API does, the
/// highest not first (`.10` is above `.9`, which is above `.7`), and the alpha of `feat-x-y`,
/// a longer name that starts with `feat-x`, the highest of all.
fn alphas_of_two_branches(releases: &Releases, programs: &Path) {
    let versions = [
        "0.5.0-alpha.feat-x-y.30",
        "0.5.0-alpha.feat-x.9",
        "0.5.0-alpha.feat-x.10",
        "0.5.0-alpha.feat-x.7",
    ];
    let made: Vec<serde_json::Value> = versions
        .iter()
        .map(|version| {
            releases.release(
                &format!("v{version}"),
                version,
                &standing_for(programs, version),
                None,
            )
        })
        .collect();
    releases.list(&made.iter().collect::<Vec<_>>());
}

#[test]
fn the_highest_alpha_of_a_branch_is_installed() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    alphas_of_two_branches(&releases, programs.path());
    let home = tempfile::tempdir().unwrap();

    let output = install(
        home.path(),
        &releases,
        &["--channel", "alpha", "--branch", "feat/x"],
    );

    assert!(output.status.success(), "{output:?}");
    let said = Command::new(home.path().join(".local/bin/flottille"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.5.0-alpha.feat-x.10 (test, 000000000000)"
    );
}

#[test]
fn install_sh_refuses_while_a_daemon_runs_and_says_to_stop_it_first() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new();
    // Started by any command, from the binary already installed.
    user.status();

    let output = Command::new("sh")
        .arg(tool("install.sh"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", user.path("home"))
        .env("XDG_RUNTIME_DIR", user.path("runtime"))
        .env("FLOTTILLE_RELEASES", releases.api())
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("`flottille daemon stop`"),
        "{output:?}"
    );
    assert!(!user.path("home/.local/bin/flottille").exists());
    assert!(
        releases.asked.lock().unwrap().is_empty(),
        "nothing was read"
    );
    // The check started no daemon of its own and stopped none: the one running still answers.
    assert!(
        std::os::unix::net::UnixStream::connect(user.path("runtime/flottille/daemon.sock")).is_ok()
    );

    user.run(&["daemon", "stop"]);
    let output = Command::new("sh")
        .arg(tool("install.sh"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", user.path("home"))
        .env("XDG_RUNTIME_DIR", user.path("runtime"))
        .env("FLOTTILLE_RELEASES", releases.api())
        .output()
        .unwrap();
    assert!(output.status.success(), "once stopped: {output:?}");
}

#[test]
fn the_publish_job_runs_no_git_and_not_inside_the_artifact() {
    // The job holding the write token: an artifact can carry a repository's layout, whose
    // configuration git would apply from the current folder (core.sshCommand, insteadOf).
    let workflow = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    let publish = &workflow[workflow.find("\n  publish:\n").expect("a publish job")..];
    for line in publish.lines().filter(|l| !l.trim_start().starts_with('#')) {
        assert!(!line.contains("working-directory"), "{line}");
        // `git` run as a command, not `…/git/ref/…` in the API's paths.
        assert!(
            !line
                .split_whitespace()
                .any(|word| word.rsplit(['(', '=', ';', '|', '&']).next() == Some("git")),
            "{line}"
        );
    }
}

/// The script of a workflow's step, the one with that `id`, as GitHub runs it.
fn step_script(workflow: &str, id: &str) -> String {
    let text = std::fs::read_to_string(tool(workflow)).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.trim() == format!("id: {id}"))
        .unwrap_or_else(|| panic!("{workflow} has no step {id}"));
    let run = at
        + lines[at..]
            .iter()
            .position(|l| l.trim() == "run: |")
            .expect("a run block");
    let indent = lines[run + 1].len() - lines[run + 1].trim_start().len();
    lines[run + 1..]
        .iter()
        .take_while(|l| l.trim().is_empty() || l.len() - l.trim_start().len() >= indent)
        .map(|l| l.get(indent..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `gh` standing for GitHub's API on a throwaway repository: the calls the `Release`
/// workflow makes, answered as GitHub answers them, a ref moved without `force` only forward.
/// Each call is noted in `gh.log` beside it.
const FAKE_GH: &str = r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$FAKE_GH_LOG"
# A call that fails as GitHub may fail, to run a workflow again after it.
if [ -n "${FAKE_GH_FAIL:-}" ] && [[ "$*" == $FAKE_GH_FAIL ]]; then echo "gh: Server Error (HTTP 500)" >&2; exit 1; fi
cd "$FAKE_GH_REPO"
# The pull requests opened, one `<base> <head>` per line.
prs=$(git rev-parse --git-dir)/fake-prs
touch "$prs"
exists() { git cat-file -e "$1^{commit}" 2> /dev/null; }
# The fields given with -f, as `field[name]`.
declare -A field=()
args=("$@")
jq= base_arg= head_arg=
for ((i = 0; i < ${#args[@]}; i++)); do
  if [ "${args[i]}" = -f ]; then kv=${args[i + 1]}; field["${kv%%=*}"]=${kv#*=}; fi
  if [ "${args[i]}" = --jq ]; then jq=${args[i + 1]}; fi
  if [ "${args[i]}" = --base ]; then base_arg=${args[i + 1]}; fi
  if [ "${args[i]}" = --head ]; then head_arg=${args[i + 1]}; fi
done
case "$1 ${2:-}" in
  "api repos/o/r/git/ref/heads/"*)
    git rev-parse --verify -q "refs/heads/${2#repos/o/r/git/ref/heads/}" || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; } ;;
  "api repos/o/r/git/ref/tags/"*)
    git rev-parse --verify -q "refs/tags/${2#repos/o/r/git/ref/tags/}" || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; } ;;
  # What `--jq '.[] | "\(.object.sha) \(.ref)"'` makes of the answer: an annotated tag's object
  # is the tag's, not its commit.
  "api repos/o/r/git/matching-refs/"*)
    git for-each-ref --format='%(objectname) %(refname)' "refs/${2#repos/o/r/git/matching-refs/}*" ;;
  "api repos/o/r/commits/"*)
    git rev-parse --verify -q "${2#repos/o/r/commits/}^{commit}" || { echo "gh: No commit found (HTTP 422)" >&2; exit 1; } ;;
  # An annotated tag's object, no ref pointing at it yet.
  "api repos/o/r/git/tags")
    [ "${field[type]:-}" = commit ] && exists "${field[object]:-}" || { echo "gh: unexpected $*" >&2; exit 2; }
    git tag -a "${field[tag]}" -m "${field[message]}" "${field[object]}"
    git rev-parse "refs/tags/${field[tag]}"
    git tag --delete "${field[tag]}" > /dev/null ;;
  # A ref made, never one moved.
  "api repos/o/r/git/refs")
    git update-ref "${field[ref]}" "${field[sha]}" "" 2> /dev/null || { echo "gh: Reference already exists (HTTP 422)" >&2; exit 1; }
    echo '{}' ;;
  # What the few `--jq` asked make of a commit.
  "api repos/o/r/git/commits/"*)
    c=${2#repos/o/r/git/commits/}
    exists "$c" || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
    case "$jq" in
      .tree.sha) git rev-parse "$c^{tree}" ;;
      ".parents[0].sha") git rev-parse "$c^" ;;
      .message) git log -1 --format=%B "$c" ;;
      *) echo "gh: unexpected $*" >&2; exit 2 ;;
    esac ;;
  "api repos/o/r/git/commits")
    git commit-tree "${field[tree]}" -p "${field["parents[]"]}" -m "${field[message]}" ;;
  # A merge into a branch, made there: its commit, or 409 when it does not merge.
  "api repos/o/r/merges")
    base=$(git rev-parse --verify -q "refs/heads/${field[base]}") || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
    tree=$(git merge-tree --write-tree "$base" "${field[head]}" 2> /dev/null) || { echo "gh: Merge conflict (HTTP 409)" >&2; exit 1; }
    sha=$(git commit-tree "$tree" -p "$base" -p "${field[head]}" -m "${field[commit_message]}")
    git update-ref "refs/heads/${field[base]}" "$sha" "$base"
    echo "$sha" ;;
  "api repos/o/r/compare/"*)
    range=${2#repos/o/r/compare/} base=${range%%...*} head=${range#*...}
    exists "$base" && exists "$head" || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
    if [ "$(git rev-parse "$base")" = "$(git rev-parse "$head")" ]; then echo identical
    elif git merge-base --is-ancestor "$base" "$head"; then echo ahead
    elif git merge-base --is-ancestor "$head" "$base"; then echo behind
    else echo diverged; fi ;;
  "api -X")
    if [ "$3" = DELETE ]; then
      ref=refs/${4#repos/o/r/git/refs/}
      git rev-parse -q --verify "$ref" > /dev/null || { echo "gh: Reference does not exist (HTTP 422)" >&2; exit 1; }
      git update-ref -d "$ref"
      exit 0
    fi
    [ "$3 $4" = "PATCH repos/o/r/git/refs/heads/main" ] && [ "$7 $8" = "-F force=false" ] || { echo "gh: unexpected $*" >&2; exit 2; }
    sha=${6#sha=}
    exists "$sha" && git merge-base --is-ancestor main "$sha" || { echo "gh: Update is not a fast forward (HTTP 422)" >&2; exit 1; }
    git update-ref refs/heads/main "$sha"
    echo '{}' ;;
  "pr create") echo "$base_arg $head_arg" >> "$prs" ;;
  # The heads of those opened, into `--base` or from `--head` when given.
  "pr list")
    while read -r base head; do
      if [ -n "$base_arg" ] && [ "$base" != "$base_arg" ]; then continue; fi
      if [ -n "$head_arg" ] && [ "$head" != "$head_arg" ]; then continue; fi
      echo "$head"
    done < "$prs" ;;
  "workflow run" | "pr comment") ;;
  *) echo "gh: unexpected $*" >&2; exit 2 ;;
esac
"#;

/// What the `Release` workflow did, run on a throwaway repository with this input.
struct Released {
    output: Output,
    calls: String,
    main: String,
}

fn run_release(repo: &Repo, commit: &str, git_ref: &str) -> Released {
    run_release_of(repo, "dev", commit, git_ref)
}

/// The `Release` workflow asked to release `commit` of `branch`.
fn run_release_of(repo: &Repo, branch: &str, commit: &str, git_ref: &str) -> Released {
    let bin = tempfile::tempdir().unwrap();
    let gh = bin.path().join("gh");
    std::fs::write(&gh, FAKE_GH).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log = bin.path().join("gh.log");
    std::fs::write(&log, "").unwrap();
    let script = bin.path().join("release.sh");
    std::fs::write(
        &script,
        step_script(".github/workflows/release.yml", "release"),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap()
    );
    // As GitHub runs a step: bash -e, the step's environment.
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-eo", "pipefail"])
        .arg(&script)
        .env("PATH", path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // Who makes the tags and the commits of the fake API.
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("FAKE_GH_REPO", repo.dir.path())
        .env("FAKE_GH_LOG", &log)
        .env("GH_REPO", "o/r")
        .env("GITHUB_REF", git_ref)
        .env("BRANCH", branch)
        .env("COMMIT", commit)
        .env("RUNNER_TEMP", bin.path())
        .output()
        .unwrap();
    Released {
        output,
        calls: std::fs::read_to_string(&log).unwrap(),
        main: repo.git(&["rev-parse", "main"]),
    }
}

/// The repository as it is: two releases made by squash, then dev moved on. On dev.
fn ready_for_a_release() -> Repo {
    let repo = Repo::new();
    repo.released_by_squash_twice();
    repo.commit("feat(tools): cut long results (#350)");
    repo.commit("fix(store): a lock (#351)");
    repo
}

#[test]
fn the_release_workflow_moves_main_forward_to_dev_s_head_and_starts_the_stable_build() {
    let repo = ready_for_a_release();
    let dev = repo.head();

    let released = run_release(&repo, "dev", "refs/heads/dev");

    assert!(released.output.status.success(), "{:?}", released.output);
    assert_eq!(released.main, dev);
    assert!(
        released.calls.contains(&format!(
            "api -X PATCH repos/o/r/git/refs/heads/main -f sha={dev} -F force=false"
        )),
        "{}",
        released.calls
    );
    assert!(
        released.calls.ends_with("workflow run ci.yml --ref main\n"),
        "{}",
        released.calls
    );
}

#[test]
fn the_release_workflow_releases_an_older_commit_of_dev() {
    let repo = ready_for_a_release();
    let older = repo.git(&["rev-parse", "HEAD~1"]);

    let released = run_release(&repo, &older, "refs/heads/dev");

    assert!(released.output.status.success(), "{:?}", released.output);
    assert_eq!(released.main, older);
}

#[test]
fn the_release_workflow_refuses_a_commit_not_on_dev() {
    let repo = ready_for_a_release();
    repo.git(&["checkout", "--quiet", "-b", "feat/x"]);
    repo.commit("feat(gui): not merged");
    let elsewhere = repo.head();
    let before = repo.git(&["rev-parse", "main"]);

    let released = run_release(&repo, &elsewhere, "refs/heads/dev");

    assert!(!released.output.status.success(), "{:?}", released.output);
    assert!(
        String::from_utf8_lossy(&released.output.stdout).contains("not a commit of dev"),
        "{:?}",
        released.output
    );
    assert_eq!(released.main, before);
    assert!(!released.calls.contains("PATCH"), "{}", released.calls);
    assert!(
        !released.calls.contains("workflow run"),
        "{}",
        released.calls
    );
}

#[test]
fn the_release_workflow_refuses_when_main_is_not_an_ancestor() {
    let repo = ready_for_a_release();
    // A commit on main that dev does not hold.
    repo.git(&["checkout", "--quiet", "main"]);
    repo.commit("fix(cli): straight on main");
    let before = repo.head();
    repo.git(&["checkout", "--quiet", "dev"]);

    let released = run_release(&repo, "dev", "refs/heads/dev");

    assert!(!released.output.status.success(), "{:?}", released.output);
    assert!(
        String::from_utf8_lossy(&released.output.stdout).contains("not an ancestor"),
        "{:?}",
        released.output
    );
    assert_eq!(released.main, before);
    assert!(!released.calls.contains("PATCH"), "{}", released.calls);
    assert!(
        !released.calls.contains("workflow run"),
        "{}",
        released.calls
    );
}

#[test]
fn the_release_workflow_refuses_an_input_that_is_not_a_commit_or_dev() {
    let repo = ready_for_a_release();
    let head = repo.head();
    for input in [
        "main",
        "dev; touch pwned",
        "$(touch pwned)",
        &head[..12],
        &head.to_uppercase(),
        "",
    ] {
        let released = run_release(&repo, input, "refs/heads/dev");

        assert!(
            !released.output.status.success(),
            "{input:?}: {:?}",
            released.output
        );
        assert_eq!(released.calls, "", "{input:?}");
    }
    assert!(!repo.dir.path().join("pwned").exists());
    // A full SHA that names no commit.
    let released = run_release(&repo, &"0".repeat(40), "refs/heads/dev");
    assert!(!released.output.status.success(), "{:?}", released.output);
    assert!(!released.calls.contains("PATCH"), "{}", released.calls);
}

#[test]
fn the_release_workflow_runs_from_dev_only() {
    let repo = ready_for_a_release();

    let released = run_release(&repo, "dev", "refs/heads/feat/x");

    assert!(!released.output.status.success(), "{:?}", released.output);
    assert_eq!(released.calls, "");
}

#[test]
fn the_release_workflow_run_again_moves_nothing_and_starts_the_build_again() {
    let repo = ready_for_a_release();
    run_release(&repo, "dev", "refs/heads/dev");

    let released = run_release(&repo, "dev", "refs/heads/dev");

    assert!(released.output.status.success(), "{:?}", released.output);
    assert!(!released.calls.contains("PATCH"), "{}", released.calls);
    assert!(
        released.calls.ends_with("workflow run ci.yml --ref main\n"),
        "{}",
        released.calls
    );
}

#[test]
fn the_release_workflow_runs_no_code_of_the_branch_and_interpolates_no_input() {
    // It holds a token that writes main: no checkout, no git, and the input reaches the script
    // through its environment only, never pasted into it by `${{ }}`.
    let workflow = std::fs::read_to_string(tool(".github/workflows/release.yml")).unwrap();
    assert!(!workflow.contains("actions/checkout"));
    assert!(!workflow.contains("force=true"));
    let script = step_script(".github/workflows/release.yml", "release");
    assert!(!script.contains("${{"), "{script}");
    for line in script.lines().filter(|l| !l.trim_start().starts_with('#')) {
        assert!(
            !line
                .split_whitespace()
                .any(|word| word.rsplit(['(', '=', ';', '|', '&']).next() == Some("git")),
            "{line}"
        );
    }
}

/// The line `stable/0.3` with its head's rc published, as CI publishes it on
/// the push (a lightweight tag, made by `gh release create`). On dev.
fn a_line_ready_for_a_release() -> Repo {
    let repo = a_line_of_0_3();
    repo.tag("v0.3.1-rc.1");
    repo.git(&["checkout", "--quiet", "dev"]);
    repo
}

#[test]
fn a_line_s_commit_released_tags_it_and_leaves_main() {
    let repo = a_line_ready_for_a_release();
    let line = repo.git(&["rev-parse", "stable/0.3"]);
    let main = repo.git(&["rev-parse", "main"]);

    let released = run_release_of(&repo, "stable/0.3", "stable/0.3", "refs/heads/dev");

    assert!(released.output.status.success(), "{:?}", released.output);
    assert_eq!(released.main, main, "main does not move for a line");
    assert!(!released.calls.contains("PATCH"), "{}", released.calls);
    // The version its rc had, on that very commit, the tag naming its line.
    assert_eq!(repo.git(&["rev-parse", "v0.3.1^{commit}"]), line);
    assert_eq!(repo.git(&["cat-file", "-t", "v0.3.1"]), "tag");
    assert_eq!(
        repo.git(&["tag", "--list", "--format=%(contents:subject)", "v0.3.1"]),
        "stable/0.3"
    );
    assert!(
        released
            .calls
            .ends_with("workflow run ci.yml --ref v0.3.1\n"),
        "{}",
        released.calls
    );
    // The line then counts from it.
    repo.git(&["checkout", "--quiet", "stable/0.3"]);
    repo.commit("fix(cli): a typo (#14)");
    assert_eq!(repo.next(&["beta", "--branch", "stable/0.3"]), "0.3.2-rc.1");
}

#[test]
fn a_line_s_older_commit_is_released_by_its_full_sha() {
    let repo = a_line_ready_for_a_release();
    let older = repo.git(&["rev-parse", "stable/0.3"]);
    repo.git(&["checkout", "--quiet", "stable/0.3"]);
    repo.commit("fix(cli): a typo (#14)");
    repo.tag("v0.3.1-rc.2");
    repo.git(&["checkout", "--quiet", "dev"]);

    let released = run_release_of(&repo, "stable/0.3", &older, "refs/heads/dev");

    assert!(released.output.status.success(), "{:?}", released.output);
    assert_eq!(repo.git(&["rev-parse", "v0.3.1^{commit}"]), older);
}

#[test]
fn a_commit_on_no_line_nor_dev_is_refused() {
    let repo = a_line_ready_for_a_release();
    let dev = repo.head();
    let tags = repo.git(&["tag", "--list"]);

    // A commit of dev is not one of the line.
    let released = run_release_of(&repo, "stable/0.3", &dev, "refs/heads/dev");
    assert!(!released.output.status.success(), "{:?}", released.output);
    assert!(
        String::from_utf8_lossy(&released.output.stdout).contains("not a commit of stable/0.3"),
        "{:?}",
        released.output
    );
    assert!(
        !released.calls.contains("workflow run"),
        "{}",
        released.calls
    );

    // A branch that is neither dev nor a line, or the head of another branch than the one named.
    for (branch, commit) in [
        ("main", dev.as_str()),
        ("feat/x", dev.as_str()),
        ("stable/0.3.1", dev.as_str()),
        ("stable/x", dev.as_str()),
        ("stable/03", dev.as_str()),
        ("stable/0.3; touch pwned", dev.as_str()),
        ("next/0.5", dev.as_str()),
        ("stable/0.3", "dev"),
        ("dev", "stable/0.3"),
    ] {
        let released = run_release_of(&repo, branch, commit, "refs/heads/dev");
        assert!(
            !released.output.status.success(),
            "{branch} {commit}: {:?}",
            released.output
        );
        assert_eq!(released.calls, "", "{branch} {commit}");
    }
    assert!(!repo.dir.path().join("pwned").exists());

    // A line that does not exist.
    let released = run_release_of(&repo, "stable/9.9", &dev, "refs/heads/dev");
    assert!(!released.output.status.success(), "{:?}", released.output);
    assert!(
        !released.calls.contains("workflow run"),
        "{}",
        released.calls
    );

    assert_eq!(repo.git(&["tag", "--list"]), tags, "no tag was made");
}

#[test]
fn a_line_s_commit_without_an_rc_is_refused() {
    // Its push run published nothing (it failed, or the line moved on before): nothing says it
    // passed `verify`, nor which version it is.
    let repo = a_line_ready_for_a_release();
    repo.git(&["checkout", "--quiet", "stable/0.3"]);
    repo.commit("fix(cli): a typo (#14)");
    repo.git(&["checkout", "--quiet", "dev"]);

    let released = run_release_of(&repo, "stable/0.3", "stable/0.3", "refs/heads/dev");

    assert!(!released.output.status.success(), "{:?}", released.output);
    assert!(
        String::from_utf8_lossy(&released.output.stdout).contains("No release candidate"),
        "{:?}",
        released.output
    );
    assert!(repo.git(&["tag", "--list", "v0.3.2"]).is_empty());
    assert!(
        !released.calls.contains("workflow run"),
        "{}",
        released.calls
    );
}

#[test]
fn a_line_s_release_run_again_tags_nothing_and_starts_the_build_again() {
    let repo = a_line_ready_for_a_release();
    run_release_of(&repo, "stable/0.3", "stable/0.3", "refs/heads/dev");
    let tag = repo.git(&["rev-parse", "v0.3.1"]);

    let released = run_release_of(&repo, "stable/0.3", "stable/0.3", "refs/heads/dev");

    assert!(released.output.status.success(), "{:?}", released.output);
    assert_eq!(repo.git(&["rev-parse", "v0.3.1"]), tag);
    assert!(
        !released.calls.contains("repos/o/r/git/refs "),
        "{}",
        released.calls
    );
    assert!(
        released
            .calls
            .ends_with("workflow run ci.yml --ref v0.3.1\n"),
        "{}",
        released.calls
    );
}

#[test]
fn a_line_s_release_whose_tag_is_on_another_commit_is_refused() {
    let repo = a_line_ready_for_a_release();
    let dev = repo.head();
    repo.git(&["tag", "v0.3.1", &dev]);

    let released = run_release_of(&repo, "stable/0.3", "stable/0.3", "refs/heads/dev");

    assert!(!released.output.status.success(), "{:?}", released.output);
    assert_eq!(
        repo.git(&["rev-parse", "v0.3.1"]),
        dev,
        "the tag is not moved"
    );
    assert!(
        !released.calls.contains("workflow run"),
        "{}",
        released.calls
    );
}

#[test]
fn the_hook_refuses_a_commit_on_a_stable_or_next_line_as_on_dev() {
    let guard = |branch: &str| {
        Command::new(tool("tools/branch-guard.sh"))
            .arg(branch)
            .output()
            .unwrap()
    };
    for branch in ["main", "dev", "stable/0.3", "stable/1.12", "next/0.5"] {
        let output = guard(branch);
        assert!(!output.status.success(), "{branch}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(branch),
            "{branch}: {output:?}"
        );
    }
    for branch in [
        "feat/930-lines",
        "fix/930-lines",
        "backport/931-0.3",
        "forward/0123456789ab-0.5",
    ] {
        let output = guard(branch);
        assert!(output.status.success(), "{branch}: {output:?}");
    }
}

/// The step `id` of a workflow run with `env` against a throwaway repository, `FAKE_GH`
/// standing for GitHub's API on it. Its output and the calls `gh` was given.
fn run_step(repo: &Repo, workflow: &str, id: &str, env: &[(&str, &str)]) -> (Output, String) {
    let bin = tempfile::tempdir().unwrap();
    let gh = bin.path().join("gh");
    std::fs::write(&gh, FAKE_GH).unwrap();
    std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let log = bin.path().join("gh.log");
    std::fs::write(&log, "").unwrap();
    let script = bin.path().join("step.sh");
    std::fs::write(&script, step_script(workflow, id)).unwrap();
    let mut command = Command::new("bash");
    command
        .args(["--noprofile", "--norc", "-eo", "pipefail"])
        .arg(&script)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("FAKE_GH_REPO", repo.dir.path())
        .env("FAKE_GH_LOG", &log)
        .env("GH_REPO", "o/r")
        .env("RUNNER_TEMP", bin.path());
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    (output, std::fs::read_to_string(&log).unwrap())
}

/// The backport workflow for the pull request #20, merged into dev by `squash`, so labelled.
fn backport(repo: &Repo, squash: &str, labels: &str) -> (Output, String) {
    run_step(
        repo,
        ".github/workflows/backport.yml",
        "backport",
        &[
            ("PR", "20"),
            ("SQUASH", squash),
            ("TITLE", "fix(cli): a typo"),
            ("LABELS", labels),
        ],
    )
}

#[test]
fn a_backport_label_cherry_picks_the_squash_onto_a_branch_of_the_line_and_opens_its_pull_request() {
    let repo = a_line_of_0_3();
    let line = repo.head();
    repo.git(&["checkout", "--quiet", "dev"]);
    repo.commit("fix(cli): a typo (#20)");
    let squash = repo.head();

    let (output, calls) = backport(&repo, &squash, r#"["type:fix", "backport 0.3"]"#);

    assert!(output.status.success(), "{output:?}");
    let branch = "backport/20-0.3";
    assert_eq!(repo.git(&["rev-parse", &format!("{branch}~1")]), line);
    assert_eq!(
        repo.git(&["rev-parse", "stable/0.3"]),
        line,
        "the line is not moved"
    );
    // The line's files and the squash's change, nothing of dev's other commits.
    assert_eq!(
        repo.git(&["diff", "--name-only", "stable/0.3", branch]),
        repo.git(&["diff", "--name-only", &format!("{squash}~1"), &squash])
    );
    let message = repo.git(&["log", "-1", "--format=%B", branch]);
    assert!(message.starts_with("fix(cli): a typo (#20)"), "{message}");
    assert!(
        message.ends_with(&format!("(cherry picked from commit {squash})")),
        "{message}"
    );
    assert_eq!(repo.git(&["branch", "--list", "backport-work/*"]), "");
    assert!(
        calls.contains(
            "pr create --base stable/0.3 --head backport/20-0.3 --title fix(cli): a typo"
        ),
        "{calls}"
    );
    assert!(
        calls.ends_with("workflow run ci.yml --ref backport/20-0.3\n"),
        "{calls}"
    );

    // Run again (another label added), it brings nothing twice.
    let (output, calls) = backport(&repo, &squash, r#"["backport 0.3", "area:gui"]"#);
    assert!(output.status.success(), "{output:?}");
    assert!(!calls.contains("pr create"), "{calls}");
}

#[test]
fn a_pull_request_without_a_backport_label_brings_nothing() {
    let repo = a_line_of_0_3();
    repo.git(&["checkout", "--quiet", "dev"]);
    repo.commit("fix(cli): a typo (#20)");
    let squash = repo.head();

    let (output, calls) = backport(
        &repo,
        &squash,
        r#"["type:fix", "backport 0.3.1", "backport"]"#,
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(calls, "");
}

#[test]
fn a_backport_that_does_not_apply_says_so_on_the_pull_request_and_opens_nothing() {
    let repo = a_line_of_0_3();
    std::fs::write(repo.dir.path().join("notes"), "the line's").unwrap();
    repo.git(&["add", "--all"]);
    repo.git(&[
        "commit",
        "--quiet",
        "-m",
        "fix(cli): the line's notes (#19)",
    ]);
    repo.git(&["checkout", "--quiet", "dev"]);
    std::fs::write(repo.dir.path().join("notes"), "dev's").unwrap();
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "--quiet", "-m", "fix(cli): dev's notes (#20)"]);
    let squash = repo.head();

    let (output, calls) = backport(&repo, &squash, r#"["backport 0.3"]"#);

    assert!(!output.status.success(), "{output:?}");
    let comment = calls
        .lines()
        .find(|l| l.starts_with("pr comment 20"))
        .unwrap_or_else(|| panic!("{calls}"));
    assert!(comment.contains("does not apply"), "{comment}");
    assert!(comment.contains(&squash), "{comment}");
    assert!(!calls.contains("pr create"), "{calls}");
    assert!(!calls.contains("workflow run"), "{calls}");
    assert_eq!(repo.git(&["branch", "--list", "backport*"]), "");
}

/// dev, and next/0.5 started from it with a feature of its own. On dev.
fn dev_with_a_next_line() -> Repo {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(specs): the specs (#20)\n\nRelease-As: 0.4.0");
    repo.git(&["checkout", "--quiet", "-b", "next/0.5"]);
    repo.commit("feat(remote): a phone (#30)");
    repo.git(&["checkout", "--quiet", "dev"]);
    repo
}

fn forward(repo: &Repo) -> (Output, String) {
    let sha = repo.git(&["rev-parse", "dev"]);
    run_step(
        repo,
        ".github/workflows/forward.yml",
        "forward",
        &[("GITHUB_SHA", &sha)],
    )
}

#[test]
fn every_commit_of_dev_is_merged_forward_into_next() {
    let repo = dev_with_a_next_line();
    let next = repo.git(&["rev-parse", "next/0.5"]);
    repo.commit("fix(cli): a typo (#21)");
    let dev = repo.head();

    let (output, calls) = forward(&repo);

    assert!(output.status.success(), "{output:?}");
    // A merge commit, next's own line first, never a squash.
    assert_eq!(repo.git(&["rev-parse", "next/0.5^1"]), next);
    assert_eq!(repo.git(&["rev-parse", "next/0.5^2"]), dev);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s", "next/0.5"]),
        "chore(repo): merge dev into next/0.5"
    );
    assert!(
        calls.ends_with("workflow run ci.yml --ref next/0.5\n"),
        "{calls}"
    );
    // next stays above dev.
    repo.git(&["checkout", "--quiet", "next/0.5"]);
    assert_eq!(repo.next(&["beta", "--branch", "next/0.5"]), "0.5.0-next.4");
    repo.git(&["checkout", "--quiet", "dev"]);

    // Run again: nothing more to merge.
    let merged = repo.git(&["rev-parse", "next/0.5"]);
    let (output, calls) = forward(&repo);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(repo.git(&["rev-parse", "next/0.5"]), merged);
    assert!(!calls.contains("merges"), "{calls}");
}

#[test]
fn a_commit_of_dev_that_does_not_merge_forward_opens_a_pull_request_with_the_conflict() {
    let repo = dev_with_a_next_line();
    repo.git(&["checkout", "--quiet", "next/0.5"]);
    std::fs::write(repo.dir.path().join("notes"), "next's").unwrap();
    repo.git(&["add", "--all"]);
    repo.git(&[
        "commit",
        "--quiet",
        "-m",
        "feat(remote): next's notes (#31)",
    ]);
    let next = repo.head();
    repo.git(&["checkout", "--quiet", "dev"]);
    std::fs::write(repo.dir.path().join("notes"), "dev's").unwrap();
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "--quiet", "-m", "fix(cli): dev's notes (#21)"]);
    let dev = repo.head();

    let (output, calls) = forward(&repo);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        repo.git(&["rev-parse", "next/0.5"]),
        next,
        "next is not moved"
    );
    let branch = format!("forward/{}-0.5", &dev[..12]);
    assert_eq!(repo.git(&["rev-parse", &branch]), dev);
    assert!(
        calls.contains(&format!(
            "pr create --base next/0.5 --head {branch} --title chore(repo): merge dev into next/0.5"
        )),
        "{calls}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("does not merge"),
        "{output:?}"
    );
    assert!(!calls.contains("workflow run"), "{calls}");

    // Run again: the branch and its pull request are there already.
    let (output, calls) = forward(&repo);
    assert!(output.status.success(), "{output:?}");
    assert!(!calls.contains("pr create"), "{calls}");
}

/// dev and next/0.5 changing the same file: dev's commit does not merge forward. On dev.
fn dev_that_does_not_merge_forward() -> Repo {
    let repo = dev_with_a_next_line();
    repo.git(&["checkout", "--quiet", "next/0.5"]);
    std::fs::write(repo.dir.path().join("notes"), "next's").unwrap();
    repo.git(&["add", "--all"]);
    repo.git(&[
        "commit",
        "--quiet",
        "-m",
        "feat(remote): next's notes (#31)",
    ]);
    repo.git(&["checkout", "--quiet", "dev"]);
    std::fs::write(repo.dir.path().join("notes"), "dev's").unwrap();
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "--quiet", "-m", "fix(cli): dev's notes (#21)"]);
    repo
}

#[test]
fn a_push_to_dev_while_a_forward_pull_request_is_open_opens_no_other() {
    let repo = dev_that_does_not_merge_forward();
    let (output, calls) = forward(&repo);
    assert!(output.status.success(), "{output:?}");
    assert!(calls.contains("pr create"), "{calls}");
    let next = repo.git(&["rev-parse", "next/0.5"]);

    // dev moves on while that pull request is open.
    repo.commit("fix(gui): a colour (#22)");
    let later = repo.head();
    let (output, calls) = forward(&repo);

    assert!(output.status.success(), "{output:?}");
    assert!(!calls.contains("pr create"), "{calls}");
    assert!(!calls.contains("merges"), "{calls}");
    assert!(!calls.contains("git/refs "), "{calls}");
    assert_eq!(repo.git(&["rev-parse", "next/0.5"]), next);
    assert_eq!(
        repo.git(&["branch", "--list", &format!("forward/{}-0.5", &later[..12])]),
        ""
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("is open"),
        "{output:?}"
    );
}

#[test]
fn a_forward_run_again_after_its_pull_request_failed_opens_it() {
    let repo = dev_that_does_not_merge_forward();
    let dev = repo.head();
    let failing = |repo: &Repo| {
        run_step(
            repo,
            ".github/workflows/forward.yml",
            "forward",
            &[("GITHUB_SHA", &dev), ("FAKE_GH_FAIL", "pr create*")],
        )
    };
    let (output, _) = failing(&repo);
    assert!(!output.status.success(), "{output:?}");
    let branch = format!("forward/{}-0.5", &dev[..12]);
    assert_eq!(
        repo.git(&["rev-parse", &branch]),
        dev,
        "the branch was made"
    );

    let (output, calls) = forward(&repo);

    assert!(output.status.success(), "{output:?}");
    assert!(
        calls.contains(&format!("pr create --base next/0.5 --head {branch}")),
        "{calls}"
    );
}

#[test]
fn a_backport_run_again_after_a_failure_finishes_it() {
    let setup = || {
        let repo = a_line_of_0_3();
        repo.git(&["checkout", "--quiet", "dev"]);
        repo.commit("fix(cli): a typo (#20)");
        let squash = repo.head();
        (repo, squash)
    };
    let failing = |repo: &Repo, squash: &str, fail: &str| {
        run_step(
            repo,
            ".github/workflows/backport.yml",
            "backport",
            &[
                ("PR", "20"),
                ("SQUASH", squash),
                ("TITLE", "fix(cli): a typo"),
                ("LABELS", r#"["backport 0.3"]"#),
                ("FAKE_GH_FAIL", fail),
            ],
        )
    };

    // The branch made, its pull request not: the run again opens it, and builds it.
    let (repo, squash) = setup();
    let (output, _) = failing(&repo, &squash, "pr create*");
    assert!(!output.status.success(), "{output:?}");
    let made = repo.git(&["rev-parse", "backport/20-0.3"]);
    let (output, calls) = backport(&repo, &squash, r#"["backport 0.3"]"#);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(repo.git(&["rev-parse", "backport/20-0.3"]), made);
    assert!(
        calls.contains("pr create --base stable/0.3 --head backport/20-0.3"),
        "{calls}"
    );
    assert!(
        calls.ends_with("workflow run ci.yml --ref backport/20-0.3\n"),
        "{calls}"
    );
    // Opened: a third run opens nothing.
    let (output, calls) = backport(&repo, &squash, r#"["backport 0.3"]"#);
    assert!(output.status.success(), "{output:?}");
    assert!(!calls.contains("pr create"), "{calls}");

    // A failure mid-pick removes its work branch, and the run again brings it.
    let (repo, squash) = setup();
    let (output, _) = failing(&repo, &squash, "api repos/o/r/merges*");
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(repo.git(&["branch", "--list", "backport*"]), "");
    // One left by a run cut short is not in the way.
    repo.git(&["branch", "backport-work/20-0.3", "stable/0.3"]);
    let (output, calls) = backport(&repo, &squash, r#"["backport 0.3"]"#);
    assert!(output.status.success(), "{output:?}");
    assert!(calls.contains("pr create"), "{calls}");
    assert_eq!(repo.git(&["branch", "--list", "backport-work/*"]), "");
}

#[test]
fn the_backport_and_forward_workflows_run_no_code_of_any_branch_with_the_least_rights() {
    for (name, id) in [("backport", "backport"), ("forward", "forward")] {
        let path = format!(".github/workflows/{name}.yml");
        let workflow = std::fs::read_to_string(tool(&path)).unwrap();
        // No checkout, no action: only `gh` through the API.
        assert!(!workflow.contains("uses:"), "{name}");
        assert!(workflow.contains("permissions: {}\n"), "{name}");
        let rights: Vec<&str> = workflow
            .lines()
            .map(str::trim)
            .filter(|l| l.ends_with(": write") || l.ends_with(": read"))
            .collect();
        assert_eq!(
            rights,
            ["contents: write", "pull-requests: write", "actions: write"],
            "{name}"
        );
        let script = step_script(&path, id);
        assert!(!script.contains("${{"), "{name}: {script}");
        assert!(!script.contains("force"), "{name}: {script}");
        for line in script.lines().filter(|l| !l.trim_start().starts_with('#')) {
            assert!(
                !line
                    .split_whitespace()
                    .any(|word| word.rsplit(['(', '=', ';', '|', '&']).next() == Some("git")),
                "{name}: {line}"
            );
        }
    }
    let backport = std::fs::read_to_string(tool(".github/workflows/backport.yml")).unwrap();
    assert!(
        backport.contains(
            "  pull_request_target:\n    types: [closed, labeled]\n    branches: [dev]\n"
        )
    );
    assert!(backport.contains("if: github.event.pull_request.merged == true"));
    let forward = std::fs::read_to_string(tool(".github/workflows/forward.yml")).unwrap();
    assert!(forward.contains("on:\n  push:\n    branches: [dev]\n"));
}

/// `ci.yml`'s step that computes the version, run as on a run of main at `sha`, in a clone of
/// the repository with its `origin`. Its outputs, or its failure.
fn version_step_on_main(repo: &Repo, sha: &str) -> (Output, String) {
    let (output, outputs, _) = version_step(repo, "main", "branch", sha);
    (output, outputs)
}

/// `ci.yml`'s step that computes the version, run as on a run of the ref `name` (a `branch` or
/// a `tag`) at `sha`, in a clone of the repository with its `origin`. Its outputs, or its
/// failure, and the changelog it wrote.
fn version_step(repo: &Repo, name: &str, kind: &str, sha: &str) -> (Output, String, String) {
    version_step_with(repo, name, kind, sha, &[])
}

/// `version_step` with more of the run's environment: a pull request's `HEAD_REF` and `HEAD_SHA`.
fn version_step_with(
    repo: &Repo,
    name: &str,
    kind: &str,
    sha: &str,
    more: &[(&str, &str)],
) -> (Output, String, String) {
    let clone = tempfile::tempdir().unwrap();
    repo.git(&[
        "clone",
        "--quiet",
        &repo.dir.path().display().to_string(),
        &clone.path().display().to_string(),
    ]);
    std::os::unix::fs::symlink(tool("tools"), clone.path().join("tools")).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let outputs = temp.path().join("outputs");
    std::fs::write(&outputs, "").unwrap();
    let script = temp.path().join("version.sh");
    std::fs::write(&script, step_script(".github/workflows/ci.yml", "version")).unwrap();
    let output = repo
        .with_gh(
            Command::new("bash")
                .args(["--noprofile", "--norc", "-eo", "pipefail"])
                .arg(&script)
                .current_dir(clone.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GITHUB_REF_NAME", name)
                .env("GITHUB_REF_TYPE", kind)
                .env("GITHUB_SHA", sha)
                .env("GITHUB_OUTPUT", &outputs)
                .env("RUNNER_TEMP", temp.path())
                .envs(more.iter().copied()),
        )
        .output()
        .unwrap();
    (
        output,
        std::fs::read_to_string(&outputs).unwrap(),
        std::fs::read_to_string(temp.path().join("changelog.md")).unwrap_or_default(),
    )
}

#[test]
fn main_moved_forward_to_dev_is_built_as_the_stable_release_of_that_commit() {
    let repo = ready_for_a_release();
    let dev = repo.head();
    repo.release_by_fast_forward("0.1.2");
    repo.git(&["tag", "--delete", "v0.1.2"]);

    let (output, outputs) = version_step_on_main(&repo, &dev);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(outputs, "channel=stable\nversion=0.1.2\ntag=v0.1.2\n");
}

#[test]
fn a_push_to_main_that_is_not_a_commit_of_dev_publishes_nothing_and_fails() {
    let repo = ready_for_a_release();
    repo.git(&["checkout", "--quiet", "main"]);
    repo.commit("fix(cli): straight on main");
    let pushed = repo.head();
    repo.git(&["checkout", "--quiet", "dev"]);

    let (output, outputs) = version_step_on_main(&repo, &pushed);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("is not a commit of dev"),
        "{output:?}"
    );
    assert_eq!(outputs, "");
}

#[test]
fn the_pushes_of_a_line_are_published_as_its_rcs() {
    let repo = a_line_of_0_3();
    let sha = repo.head();
    repo.git(&["checkout", "--quiet", "dev"]);

    let (output, outputs, changelog) = version_step(&repo, "stable/0.3", "branch", &sha);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(outputs, "channel=rc\nversion=0.3.1-rc.1\ntag=v0.3.1-rc.1\n");
    assert_eq!(
        changelog,
        "### All changes\n\n#### Fixes\n\n- **store**: a lock (#13)\n"
    );
}

/// The line `stable/0.3` at 0.3.1, then a fix whose rc was published and that
/// the `Release` workflow tagged `v0.3.2`. On dev; the commit tagged.
fn a_line_tagged_for_its_release() -> (Repo, String) {
    let repo = a_line_of_0_3();
    repo.line_release("0.3.1");
    repo.commit("fix(cli): a typo (#14)");
    repo.tag("v0.3.2-rc.1");
    repo.line_release("0.3.2");
    let sha = repo.head();
    repo.git(&["checkout", "--quiet", "dev"]);
    (repo, sha)
}

#[test]
fn a_line_s_tag_is_built_as_its_stable_release_with_the_line_s_changelog() {
    let (repo, sha) = a_line_tagged_for_its_release();

    let (output, outputs, changelog) = version_step(&repo, "v0.3.2", "tag", &sha);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(outputs, "channel=stable\nversion=0.3.2\ntag=v0.3.2\n");
    assert_eq!(
        changelog,
        "### All changes\n\n#### Fixes\n\n- **cli**: a typo (#14)\n"
    );
}

#[test]
fn a_tag_not_on_its_line_publishes_nothing_and_fails() {
    let (repo, _) = a_line_tagged_for_its_release();
    // A tag of the line's form on a commit of dev, with a candidate of that version beside it.
    let dev = repo.head();
    repo.tag("v0.3.3-rc.1");
    repo.line_release("0.3.3");

    let (output, outputs, _) = version_step(&repo, "v0.3.3", "tag", &dev);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("not a commit of stable/0.3"),
        "{output:?}"
    );
    assert_eq!(outputs, "");
}

#[test]
fn a_tag_without_its_rc_publishes_nothing_and_fails() {
    let (repo, _) = a_line_tagged_for_its_release();
    repo.git(&["checkout", "--quiet", "stable/0.3"]);
    repo.commit("fix(gui): a colour (#15)");
    repo.line_release("0.3.3");
    let sha = repo.head();
    repo.git(&["checkout", "--quiet", "dev"]);

    let (output, outputs, _) = version_step(&repo, "v0.3.3", "tag", &sha);

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("has no release candidate of"),
        "{output:?}"
    );
    assert_eq!(outputs, "");
}

#[test]
fn every_release_title_has_a_v_before_its_version() {
    // `Flottille v0.1.2-beta.98`, of every channel: the one title the publish job writes, made
    // and edited with it.
    let script = step_script(".github/workflows/ci.yml", "publish");
    assert!(script.contains("title=\"Flottille v$VERSION\""), "{script}");
    assert_eq!(script.matches("title=").count(), 1, "{script}");
    assert_eq!(
        script.matches("--title \"$title\"").count(),
        script.matches("--title").count(),
        "{script}"
    );
}

#[test]
fn the_branch_name_is_sanitised_the_same_way_everywhere() {
    // Each byte but ASCII letters, digits and dashes is a dash: `é` is two.
    let branch = "feat/é_x";
    let slug = "feat----x";

    let repo = Repo::new();
    repo.commit("feat(core): start");
    assert_eq!(
        repo.next(&["alpha", "--branch", branch]),
        "0.0.1-alpha.feat----x.1"
    );

    // The alpha of that name is found by both: nothing else of the list is taken.
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let alpha = releases.release(
        &format!("v0.9.0-alpha.{slug}.1"),
        &format!("0.9.0-alpha.{slug}.1"),
        &standing_for(programs.path(), &format!("0.9.0-alpha.{slug}.1")),
        None,
    );
    let other = releases.release(
        "v0.9.0-alpha.feat-x.9",
        "0.9.0-alpha.feat-x.9",
        &standing_for(programs.path(), "0.9.0-alpha.feat-x.9"),
        None,
    );
    releases.list(&[&other, &alpha]);
    let home = tempfile::tempdir().unwrap();
    let output = install(
        home.path(),
        &releases,
        &["--channel", "alpha", "--branch", branch],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        installed_version(home.path()),
        format!("flottille 0.9.0-alpha.{slug}.1 (test, 000000000000)")
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let output = user
        .command_through(
            &binary,
            &["update", "--channel", "alpha", "--branch", branch],
        )
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        format!("flottille 0.9.0-alpha.{slug}.1 (test, 000000000000)")
    );

    // The workflows have no test of their own: they say the same rule, word for word.
    let text = std::fs::read_to_string(tool(".github/workflows/alpha-cleanup.yml")).unwrap();
    assert!(
        text.contains("LC_ALL=C tr -c 'A-Za-z0-9-' '-'"),
        "the cleanup sanitises the branch another way"
    );
}

// `flottille update`.

/// Flottille's binary in a folder of its own, the one `update` replaces.
pub(crate) struct Installed {
    _folder: tempfile::TempDir,
    path: PathBuf,
}

impl std::ops::Deref for Installed {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for Installed {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<std::ffi::OsStr> for Installed {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.path.as_os_str()
    }
}

/// The binary `update` replaces, linked to the real one rather than copied: it weighs a gigabyte
/// in a debug build, and /tmp may be a small tmpfs. `update` renames the new binary over this
/// name, so the real one is left as it is. The folder is where cargo keeps the tests' scratch
/// files, on the file system of the build: a link cannot cross to /tmp.
pub(crate) fn installed() -> Installed {
    let folder = tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let path = folder.path().join("flottille");
    if std::fs::hard_link(env!("CARGO_BIN_EXE_flottille"), &path).is_err() {
        // Copied by `cp`, not by this process: a file it held open for writing would be
        // inherited by what another test spawns at that moment, and running it would fail with
        // "Text file busy".
        let copied = Command::new("cp")
            .arg(env!("CARGO_BIN_EXE_flottille"))
            .arg(&path)
            .status()
            .unwrap();
        assert!(copied.success());
    }
    Installed {
        _folder: folder,
        path,
    }
}

#[test]
fn the_installed_binary_is_a_link_to_the_real_one_not_a_copy() {
    use std::os::unix::fs::MetadataExt;

    let binary = installed();

    let (linked, real) = (
        std::fs::metadata(&binary).unwrap(),
        std::fs::metadata(env!("CARGO_BIN_EXE_flottille")).unwrap(),
    );
    assert_eq!((linked.dev(), linked.ino()), (real.dev(), real.ino()));
}

#[test]
fn flottille_update_replaces_the_binary_from_a_local_fake_release() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();

    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("0.4.1-beta.3"),
        "{output:?}"
    );
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.4.1-beta.3 (test, 000000000000)"
    );
    let left: Vec<String> = std::fs::read_dir(binary.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["flottille"]);
}

#[test]
fn flottille_update_refuses_a_wrong_sum_and_keeps_the_binary() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let stable = releases.release(
        "v0.4.0",
        "0.4.0",
        &standing_for(programs.path(), "0.4.0"),
        Some(&"0".repeat(64)),
    );
    releases.put(
        "/repos/o/r/releases/latest",
        stable.to_string().into_bytes(),
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let before = sha256(&binary);

    let output = user
        .command_through(&binary, &["update", "--channel", "stable"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("SHA-256"),
        "{output:?}"
    );
    assert_eq!(sha256(&binary), before);
}

#[test]
fn flottille_update_refuses_while_an_agent_runs_and_says_how_to_stop_it() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::with_fake_agent().with_env("FLOTTILLE_RELEASES", &releases.api());
    user.fake_script(
        json!({"turns": [{"when": "work long", "do": [{"wait": 30000}, {"say": "done"}]}]}),
    );
    let folder = user.path("work/app");
    user.git_init(&folder);
    user.git(
        &folder,
        &["commit", "--quiet", "--allow-empty", "-m", "start"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    // Summed before the agent starts: its turn lasts 30 s, and the sum of a debug binary can
    // take that long on a slow runner.
    let binary = installed();
    let before = sha256(&binary);
    let agent = user
        .call(
            "agent.start",
            json!({"project": "app", "message": "work long"}),
        )
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while user.status()["agents"]["running"] != 1 {
        assert!(std::time::Instant::now() < deadline, "the agent never ran");
        std::thread::sleep(Duration::from_millis(50));
    }

    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("flottille daemon stop"), "{said}");
    assert_eq!(
        user.status()["agents"]["running"],
        1,
        "{agent} was left alone"
    );
    assert_eq!(sha256(&binary), before);
    assert!(
        !releases
            .asked
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.contains("/download/")),
        "nothing was downloaded"
    );
}

#[test]
fn flottille_update_refuses_while_a_daemon_runs_and_says_to_stop_it_first() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    // Started by any command; no agent runs.
    user.status();
    let binary = installed();
    let before = sha256(&binary);

    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("`flottille daemon stop`"),
        "{output:?}"
    );
    assert_eq!(sha256(&binary), before);
    assert!(
        releases.asked.lock().unwrap().is_empty(),
        "nothing was read"
    );
}

#[test]
fn update_takes_the_most_recently_published_build_of_its_channel() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    // Listed in no order of publication.
    betas(
        &releases,
        programs.path(),
        &[
            ("0.4.1-beta.2", "2026-11-04T10:20:00Z"),
            ("0.4.1-beta.10", "2026-11-04T11:00:00Z"),
            ("0.4.1-beta.1", "2026-11-04T09:00:00Z"),
        ],
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();

    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.4.1-beta.10 (test, 000000000000)"
    );
}

#[test]
fn a_stray_higher_version_published_earlier_is_not_taken() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    betas(&releases, programs.path(), &STRAY);
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();

    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.1.2-beta.15 (test, 000000000000)"
    );
}

#[test]
fn update_says_when_the_latest_of_a_channel_is_lower_than_this_version() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    betas(
        &releases,
        programs.path(),
        &[
            ("0.0.2-beta.9", "2026-11-04T09:00:00Z"),
            ("0.0.1-beta.3", "2026-11-04T10:00:00Z"),
        ],
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let before = sha256(&binary);

    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("0.0.1-beta.3 is the latest of beta"),
        "{said}"
    );
    assert!(said.contains("--force"), "{said}");
    assert_eq!(sha256(&binary), before);

    let output = user
        .command_through(&binary, &["update", "--channel", "beta", "--force"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.0.1-beta.3 (test, 000000000000)"
    );
}

#[test]
fn update_moves_to_the_alpha_of_a_branch_and_back_to_beta() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let version_of = |binary: &Path| {
        let said = Command::new(binary).arg("--version").output().unwrap();
        String::from_utf8_lossy(&said.stdout).trim().to_owned()
    };

    let binary = installed();
    let output = user
        .command_through(
            &binary,
            &[
                "update",
                "--channel",
                "alpha",
                "--branch",
                "feat/46-packages",
            ],
        )
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        version_of(&binary),
        "flottille 0.5.0-alpha.feat-46-packages.7 (test, 000000000000)"
    );

    let binary = installed();
    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        version_of(&binary),
        "flottille 0.4.1-beta.3 (test, 000000000000)"
    );
}

#[test]
fn update_remembers_the_channel_last_chosen() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    // What a plain `flottille update` read, from a binary freshly built (so, not this one's
    // own memory: the user's state folder is what keeps the choice).
    let plain_update_reads = || {
        releases.asked.lock().unwrap().clear();
        let binary = installed();
        let output = user.command_through(&binary, &["update"]).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let asked = releases.asked.lock().unwrap().clone();
        asked
            .into_iter()
            .filter(|path| path.contains("/releases"))
            .collect::<Vec<_>>()
    };
    let choose = |args: &[&str]| {
        let binary = installed();
        let output = user.command_through(&binary, args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    };

    // Nothing chosen: the channel of the build, and a local build takes stable.
    assert_eq!(plain_update_reads(), ["/repos/o/r/releases/latest"]);

    choose(&["update", "--channel", "beta"]);
    assert_eq!(plain_update_reads(), ["/repos/o/r/releases?per_page=100"]);

    choose(&[
        "update",
        "--channel",
        "alpha",
        "--branch",
        "feat/46-packages",
    ]);
    assert_eq!(plain_update_reads(), ["/repos/o/r/releases?per_page=100"]);

    choose(&["update", "--channel", "stable"]);
    assert_eq!(plain_update_reads(), ["/repos/o/r/releases/latest"]);
}

#[test]
fn a_plain_update_says_which_channel_it_follows() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let plain_update = || {
        let binary = installed();
        let output = user.command_through(&binary, &["update"]).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let choose = |args: &[&str]| {
        let binary = installed();
        let output = user.command_through(&binary, args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    };

    // Nothing remembered: no channel is said to be followed.
    assert!(!plain_update().contains("Following"));

    choose(&[
        "update",
        "--channel",
        "alpha",
        "--branch",
        "feat/46-packages",
    ]);
    assert!(
        plain_update().starts_with(
            "Following the channel last chosen: alpha (feat/46-packages); --channel beta goes back to dev's builds.\n"
        )
    );

    choose(&["update", "--channel", "beta"]);
    assert!(plain_update().starts_with("Following the channel last chosen: beta.\n"));
}

#[test]
fn a_remembered_alpha_without_a_release_says_how_to_leave_it() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let output = user
        .command_through(
            &binary,
            &[
                "update",
                "--channel",
                "alpha",
                "--branch",
                "feat/46-packages",
            ],
        )
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    // The branch is merged and its alpha deleted with it.
    releases.list(&[]);

    let binary = installed();
    let output = user.command_through(&binary, &["update"]).output().unwrap();

    assert!(!output.status.success(), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("no alpha release for feat/46-packages"),
        "{said}"
    );
    assert!(
        said.contains("--channel beta (or another --branch) chooses another channel."),
        "{said}"
    );
}

#[test]
fn a_corrupt_channel_memory_is_read_as_none() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let memory = user.path("state/flottille");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join("update-channel.json"), b"{\"channel\": \"alp").unwrap();
    releases.asked.lock().unwrap().clear();
    let binary = installed();

    let output = user.command_through(&binary, &["update"]).output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("Following"),
        "{output:?}"
    );
    // The channel of the build (a local build takes stable), not the broken memory's.
    let asked: Vec<String> = releases
        .asked
        .lock()
        .unwrap()
        .iter()
        .filter(|path| path.contains("/releases"))
        .cloned()
        .collect();
    assert_eq!(asked, ["/repos/o/r/releases/latest"]);
}

#[test]
fn the_channel_memory_is_replaced_whole_with_no_file_left_beside_it() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let choose = |args: &[&str]| {
        let binary = installed();
        let output = user.command_through(&binary, args).output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    };

    choose(&[
        "update",
        "--channel",
        "alpha",
        "--branch",
        "feat/46-packages",
    ]);
    choose(&["update", "--channel", "beta"]);

    let memory = user.path("state/flottille");
    assert_eq!(common::names_in(&memory), ["update-channel.json"]);
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(memory.join("update-channel.json")).unwrap())
            .unwrap();
    assert_eq!(saved["channel"], "beta");
    assert!(saved["branch"].is_null(), "{saved}");
}

#[test]
fn update_does_not_offer_a_draft() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let published = releases.release(
        "v0.4.1-beta.3",
        "0.4.1-beta.3",
        &standing_for(programs.path(), "0.4.1-beta.3"),
        None,
    );
    let mut draft = releases.release(
        "v0.4.1-beta.4",
        "0.4.1-beta.4",
        &standing_for(programs.path(), "0.4.1-beta.4"),
        None,
    );
    draft["published_at"] = json!(null);
    // The draft is listed after the release published last: it takes no date of that one.
    releases.list_as_github(&[&published, &draft]);
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());

    let binary = installed();
    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.4.1-beta.3 (test, 000000000000)"
    );

    // With no release published, the draft alone is not offered.
    releases.list_as_github(&[&draft]);
    let binary = installed();
    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("there is no beta release"),
        "{output:?}"
    );
}

#[test]
fn update_installs_an_exact_version() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());

    // Of any channel; neither the latest of one nor, for the alpha, a branch to give.
    for version in ["0.4.0", "0.4.1-beta.3", "0.5.0-alpha.feat-46-packages.7"] {
        let binary = installed();
        let output = user
            .command_through(&binary, &["update", "--version", version])
            .output()
            .unwrap();
        assert!(output.status.success(), "{version}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(version),
            "{output:?}"
        );
        let said = Command::new(&binary).arg("--version").output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&said.stdout).trim(),
            format!("flottille {version} (test, 000000000000)")
        );
    }

    // A version is not a channel, and it does not change the channel remembered.
    let binary = installed();
    let output = user
        .command_through(
            &binary,
            &["update", "--version", "0.4.0", "--channel", "beta"],
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    releases.asked.lock().unwrap().clear();
    let output = user.command_through(&binary, &["update"]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        releases.asked.lock().unwrap()[0],
        "/repos/o/r/releases/latest",
        "the version installed set no channel"
    );
}

#[test]
fn an_unknown_version_is_refused_with_close_ones() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    betas(
        &releases,
        programs.path(),
        &[
            ("0.0.7", "2026-11-04T09:00:00Z"),
            ("0.4.1-beta.2", "2026-11-04T09:10:00Z"),
            ("0.4.1-beta.3", "2026-11-04T09:20:00Z"),
            ("0.4.1-beta.5", "2026-11-04T09:30:00Z"),
            ("0.4.1-beta.6", "2026-11-04T09:40:00Z"),
        ],
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let before = sha256(&binary);

    let output = user
        .command_through(&binary, &["update", "--version", "0.4.1-beta.4"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("no release 0.4.1-beta.4"), "{said}");
    for close in ["0.4.1-beta.3", "0.4.1-beta.5"] {
        assert!(said.contains(close), "{said}");
    }
    assert!(!said.contains("0.0.7"), "{said}");
    assert_eq!(sha256(&binary), before);
    assert!(
        releases
            .asked
            .lock()
            .unwrap()
            .iter()
            .all(|path| !path.contains("/download/")),
        "something was downloaded"
    );

    // Something that is not a version is refused before the releases are read.
    releases.asked.lock().unwrap().clear();
    for text in ["latest", "0.4", "v0.4.0"] {
        let output = user
            .command_through(&binary, &["update", "--version", text])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{text}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("not a version"),
            "{text}: {output:?}"
        );
    }
    assert!(releases.asked.lock().unwrap().is_empty());
}

#[test]
fn going_to_a_lower_version_needs_force() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    betas(
        &releases,
        programs.path(),
        &[("0.0.1", "2026-11-04T09:00:00Z")],
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let before = sha256(&binary);

    let output = user
        .command_through(&binary, &["update", "--version", "0.0.1"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("0.0.1 is older than this"), "{said}");
    assert!(said.contains("--force"), "{said}");
    // It says what going back does to the store.
    assert!(said.contains("not migrated down"), "{said}");
    assert_eq!(sha256(&binary), before);

    let output = user
        .command_through(&binary, &["update", "--version", "0.0.1", "--force"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("not migrated down"),
        "{output:?}"
    );
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.0.1 (test, 000000000000)"
    );
}

/// The version this test build says, `0.1.0` in `flottille 0.1.0 (local, …)`.
pub(crate) fn this_version() -> String {
    let said = Command::new(env!("CARGO_BIN_EXE_flottille"))
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8_lossy(&said.stdout)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned()
}

/// A stable release at `version` only.
pub(crate) fn stable_at(releases: &Releases, programs: &Path, version: &str) {
    let stable = releases.release(
        &format!("v{version}"),
        version,
        &standing_for(programs, version),
        None,
    );
    releases.put(
        "/repos/o/r/releases/latest",
        stable.to_string().into_bytes(),
    );
}

#[test]
fn flottille_update_says_it_is_already_on_that_version_and_reinstalls_only_with_force() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    let version = this_version();
    stable_at(&releases, programs.path(), &version);
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let before = sha256(&binary);

    let output = user
        .command_through(&binary, &["update", "--channel", "stable"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&format!("already {version}")),
        "{output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("--force"),
        "{output:?}"
    );
    assert_eq!(sha256(&binary), before);

    let output = user
        .command_through(&binary, &["update", "--channel", "stable", "--force"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_ne!(sha256(&binary), before);
}

#[test]
fn flottille_update_refuses_an_older_version_without_force() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    stable_at(&releases, programs.path(), "0.0.1");
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    let before = sha256(&binary);

    let output = user
        .command_through(&binary, &["update", "--channel", "stable"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("0.0.1 is the latest of stable"), "{said}");
    assert!(said.contains("--force"), "{said}");
    assert_eq!(sha256(&binary), before);

    let output = user
        .command_through(&binary, &["update", "--channel", "stable", "--force"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.0.1 (test, 000000000000)"
    );
}

#[test]
fn flottille_update_says_a_network_failure_is_not_a_missing_release() {
    // Nothing listens on that port.
    let api = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/repos/o/r", listener.local_addr().unwrap())
    };
    let user = User::new().with_env("FLOTTILLE_RELEASES", &api);
    let binary = installed();

    for channel in ["stable", "beta", "alpha"] {
        let mut args = vec!["update", "--channel", channel];
        if channel == "alpha" {
            args.extend(["--branch", "feat/x"]);
        }
        let output = user.command_through(&binary, &args).output().unwrap();

        assert!(!output.status.success(), "{channel}: {output:?}");
        let said = String::from_utf8_lossy(&output.stderr);
        assert!(said.contains("cannot read"), "{channel}: {said}");
        assert!(!said.contains("there is no"), "{channel}: {said}");
    }
}

#[test]
fn flottille_update_refuses_a_branch_without_the_alpha_channel() {
    let releases = Releases::serve();
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();

    for args in [
        vec!["update", "--branch", "feat/x"],
        vec!["update", "--channel", "beta", "--branch", "feat/x"],
    ] {
        let output = user.command_through(&binary, &args).output().unwrap();

        assert!(!output.status.success(), "{args:?}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("--channel alpha"),
            "{args:?}: {output:?}"
        );
    }
    assert!(
        releases.asked.lock().unwrap().is_empty(),
        "nothing was read"
    );
}

#[test]
fn flottille_update_takes_the_highest_alpha_of_a_branch() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    alphas_of_two_branches(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();

    let output = user
        .command_through(
            &binary,
            &["update", "--channel", "alpha", "--branch", "feat/x"],
        )
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let said = Command::new(&binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.5.0-alpha.feat-x.10 (test, 000000000000)"
    );
}

#[test]
fn an_alpha_of_another_branch_with_a_longer_name_is_not_taken() {
    // `feat/x-y` has the highest alpha of the list; `feat/x` and `feat/x-y` each take their own.
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    alphas_of_two_branches(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());

    for (branch, version) in [
        ("feat/x", "0.5.0-alpha.feat-x.10"),
        ("feat/x-y", "0.5.0-alpha.feat-x-y.30"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let output = install(
            home.path(),
            &releases,
            &["--channel", "alpha", "--branch", branch],
        );
        assert!(output.status.success(), "{branch}: {output:?}");
        assert_eq!(
            installed_version(home.path()),
            format!("flottille {version} (test, 000000000000)")
        );
        let binary = installed();
        let output = user
            .command_through(
                &binary,
                &["update", "--channel", "alpha", "--branch", branch],
            )
            .output()
            .unwrap();
        assert!(output.status.success(), "{branch}: {output:?}");
        let said = Command::new(&binary).arg("--version").output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&said.stdout).trim(),
            format!("flottille {version} (test, 000000000000)")
        );
    }

    // A branch whose own alphas are gone has none, whatever the longer names hold.
    let only_longer = Releases::serve();
    let longer = only_longer.release(
        "v0.5.0-alpha.feat-x-y.30",
        "0.5.0-alpha.feat-x-y.30",
        &standing_for(programs.path(), "0.5.0-alpha.feat-x-y.30"),
        None,
    );
    only_longer.list(&[&longer]);
    let home = tempfile::tempdir().unwrap();
    let output = install(
        home.path(),
        &only_longer,
        &["--channel", "alpha", "--branch", "feat/x"],
    );
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no alpha release for feat/x"),
        "{output:?}"
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &only_longer.api());
    let output = user
        .command_through(
            &installed(),
            &["update", "--channel", "alpha", "--branch", "feat/x"],
        )
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no alpha release for feat/x"),
        "{output:?}"
    );
}

#[test]
fn an_exact_alpha_version_is_read_under_its_own_tag() {
    // Each alpha is its own release, tagged like a beta: `--version` asks for `v<version>`.
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    alphas_of_two_branches(&releases, programs.path());
    let home = tempfile::tempdir().unwrap();

    let output = install(
        home.path(),
        &releases,
        &["--version", "0.5.0-alpha.feat-x.7"],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        installed_version(home.path()),
        "flottille 0.5.0-alpha.feat-x.7 (test, 000000000000)"
    );
    assert!(
        releases
            .asked
            .lock()
            .unwrap()
            .contains(&"/repos/o/r/releases/tags/v0.5.0-alpha.feat-x.7".to_owned())
    );
}

// The workflows' scripts, run with a `gh` of their own.

/// A `gh` that keeps the published tags in `$FAKE_GH_STATE` (one per line, `draft:` before a
/// draft's) and notes each call in `$FAKE_GH_LOG`: what `publish` and `alpha-cleanup` ask of it.
const FAKE_RELEASES_GH: &str = r#"#!/usr/bin/env bash
set -euo pipefail
echo "$*" >> "$FAKE_GH_LOG"
touch "$FAKE_GH_STATE"
case "$1 $2" in
  "api repos/o/r/git/ref/heads/"* | "api repos/o/r/commits/"*) echo "$FAKE_TIP" ;;
  # The tag of the release marked latest, none, or the API failing.
  "api repos/o/r/releases/latest")
    case "${FAKE_LATEST:-}" in
      "") echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
      error) echo "gh: Server Error (HTTP 502)" >&2; exit 1 ;;
      *) echo "$FAKE_LATEST" ;;
    esac ;;
  "release view")
    tag=$3
    if grep -qx "$tag" "$FAKE_GH_STATE"; then echo false
    elif grep -qx "draft:$tag" "$FAKE_GH_STATE"; then echo true
    else echo "release not found" >&2; exit 1; fi ;;
  "release create") echo "$3" >> "$FAKE_GH_STATE" ;;
  "release delete") sed -i "/^\(draft:\)\?$3\$/d" "$FAKE_GH_STATE" ;;
  "release list") sed 's/^draft://' "$FAKE_GH_STATE" ;;
  *) echo "gh: unexpected $*" >&2; exit 2 ;;
esac
"#;

/// A throwaway place for the scripts of a workflow to run in, with `FAKE_RELEASES_GH` as `gh`.
struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    fn new() -> Fake {
        let fake = Fake {
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir_all(fake.dir.path().join("bin")).unwrap();
        let gh = fake.dir.path().join("bin/gh");
        std::fs::write(&gh, FAKE_RELEASES_GH).unwrap();
        std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        // The artifact `publish` downloads.
        std::fs::create_dir_all(fake.dir.path().join("release/assets")).unwrap();
        std::fs::write(fake.dir.path().join("release/notes.md"), "notes").unwrap();
        for name in ["a.tar.gz", "a.tar.gz.sha256"] {
            std::fs::write(fake.dir.path().join("release/assets").join(name), "x").unwrap();
        }
        fake
    }

    fn state(&self, tags: &[&str]) {
        std::fs::write(self.dir.path().join("state"), tags.join("\n") + "\n").unwrap();
    }

    fn tags(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("state"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The calls `gh` was given, one per line.
    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("log")).unwrap_or_default()
    }

    fn run(&self, script: &str, env: &[(&str, &str)]) -> Output {
        let mut command = Command::new("bash");
        command
            .args(["-e", "-c", script])
            .current_dir(self.dir.path())
            .env_clear()
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.dir.path().join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("RUNNER_TEMP", self.dir.path())
            .env("GH_REPO", "o/r")
            .env("GITHUB_REPOSITORY", "o/r")
            .env("FAKE_GH_LOG", self.dir.path().join("log"))
            .env("FAKE_GH_STATE", self.dir.path().join("state"));
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    /// `publish` for a push of `branch` at its tip, building this version.
    fn publish(&self, channel: &str, branch: &str, version: &str) -> Output {
        self.run(
            &step_script(".github/workflows/ci.yml", "publish"),
            &[
                ("GH_TOKEN", "unused"),
                ("CHANNEL", channel),
                ("VERSION", version),
                ("TAG", &format!("v{version}")),
                ("GITHUB_REF_NAME", branch),
                ("GITHUB_SHA", "abc123"),
                ("FAKE_TIP", "abc123"),
            ],
        )
    }
}

#[test]
fn each_push_of_a_branch_publishes_its_own_prerelease_and_never_replaces_one() {
    let fake = Fake::new();
    let branch = "fix/666-alphas";

    for n in [1, 2] {
        let version = format!("0.2.0-alpha.fix-666-alphas.{n}");
        let output = fake.publish("alpha", branch, &version);
        assert!(output.status.success(), "{output:?}");
    }

    assert_eq!(
        fake.tags(),
        [
            "v0.2.0-alpha.fix-666-alphas.1",
            "v0.2.0-alpha.fix-666-alphas.2"
        ]
    );
    let log = fake.log();
    let created: Vec<&str> = log
        .lines()
        .filter(|l| l.starts_with("release create"))
        .collect();
    assert_eq!(created.len(), 2, "{log}");
    for line in created {
        // Its files go with it in the one call: an immutable release takes none afterwards.
        assert!(line.contains("release/assets/"), "{line}");
        assert!(line.contains("--prerelease --latest=false"), "{line}");
        assert!(line.contains("--target abc123"), "{line}");
    }
    // Nothing is moved, uploaded over, edited or removed.
    for forbidden in [
        "upload",
        "edit",
        "delete-asset",
        "-X PATCH",
        "git/refs/tags",
    ] {
        assert!(!log.contains(forbidden), "{forbidden}: {log}");
    }
}

#[test]
fn an_alpha_already_published_is_not_published_again() {
    let fake = Fake::new();
    fake.state(&["v0.2.0-alpha.fix-666-alphas.1"]);

    let output = fake.publish("alpha", "fix/666-alphas", "0.2.0-alpha.fix-666-alphas.1");

    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("already published"),
        "{output:?}"
    );
    assert!(!fake.log().contains("release create"), "{}", fake.log());
    assert_eq!(fake.tags(), ["v0.2.0-alpha.fix-666-alphas.1"]);
}

#[test]
fn a_draft_left_by_a_failed_upload_is_made_again_for_an_alpha() {
    let fake = Fake::new();
    fake.state(&["draft:v0.2.0-alpha.fix-666-alphas.1"]);

    let output = fake.publish("alpha", "fix/666-alphas", "0.2.0-alpha.fix-666-alphas.1");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(fake.tags(), ["v0.2.0-alpha.fix-666-alphas.1"]);
}

#[test]
fn a_branch_that_moved_on_publishes_nothing() {
    let fake = Fake::new();
    let script = step_script(".github/workflows/ci.yml", "publish");
    let output = fake.run(
        &script,
        &[
            ("CHANNEL", "alpha"),
            ("VERSION", "0.2.0-alpha.x.1"),
            ("TAG", "v0.2.0-alpha.x.1"),
            ("GITHUB_REF_NAME", "fix/x"),
            ("GITHUB_SHA", "abc123"),
            ("FAKE_TIP", "def456"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    assert!(!fake.log().contains("release create"), "{}", fake.log());
}

/// The step of `verified-on-dev` on main at `abc123`, with a `gh` that answers the query of the
/// runs with `count` (`None`: the API fails): what it wrote to `$GITHUB_OUTPUT`, and the query.
fn verified_on_dev(count: Option<u32>) -> (Output, String, String) {
    verified_on("main", "branch", count)
}

/// The step of `verified-on-dev` on the ref `name` (a `branch` or a `tag`) at `abc123`.
fn verified_on(name: &str, kind: &str, count: Option<u32>) -> (Output, String, String) {
    let fake = Fake::new();
    let answer = match count {
        Some(count) => format!("echo {count}"),
        None => "echo 'gh: Server Error (HTTP 502)' >&2; exit 1".to_owned(),
    };
    let gh = fake.dir.path().join("bin/gh");
    std::fs::write(
        &gh,
        format!("#!/usr/bin/env bash\necho \"$*\" >> \"$FAKE_GH_LOG\"\n{answer}\n"),
    )
    .unwrap();
    let outputs = fake.dir.path().join("outputs");
    let output = fake.run(
        &step_script(".github/workflows/ci.yml", "dev"),
        &[
            ("GH_TOKEN", "unused"),
            ("GITHUB_SHA", "abc123"),
            ("GITHUB_REF_NAME", name),
            ("GITHUB_REF_TYPE", kind),
            ("GITHUB_OUTPUT", outputs.to_str().unwrap()),
        ],
    );
    let written = std::fs::read_to_string(outputs).unwrap_or_default();
    (output, written, fake.log())
}

#[test]
fn main_skips_verify_for_a_commit_whose_push_run_on_dev_succeeded() {
    let (output, written, log) = verified_on_dev(Some(1));

    assert!(output.status.success(), "{output:?}");
    assert_eq!(written, "verified=true\n");
    // Only a run of this very workflow, pushed to dev, for this commit, that succeeded.
    let query = log.lines().next().unwrap();
    for part in [
        "repos/o/r/actions/workflows/ci.yml/runs?",
        "head_sha=abc123",
        "event=push",
        "branch=dev",
        "status=success",
    ] {
        assert!(query.contains(part), "{part}: {query}");
    }
}

#[test]
fn main_runs_verify_itself_when_no_push_run_on_dev_succeeded() {
    let (output, written, _) = verified_on_dev(Some(0));

    assert!(output.status.success(), "{output:?}");
    assert_eq!(written, "");
}

#[test]
fn main_runs_verify_itself_when_the_api_fails() {
    // A failure says nothing, and fails nothing: `verify` runs, as on any commit.
    let (output, written, _) = verified_on_dev(None);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(written, "");
}

#[test]
fn a_line_s_release_skips_verify_for_a_commit_whose_push_run_on_its_line_succeeded() {
    let (output, written, log) = verified_on("v0.3.2", "tag", Some(1));

    assert!(output.status.success(), "{output:?}");
    assert_eq!(written, "verified=true\n");
    let query = log.lines().next().unwrap();
    for part in [
        "head_sha=abc123",
        "event=push",
        "branch=stable/0.3",
        "status=success",
    ] {
        assert!(query.contains(part), "{part}: {query}");
    }

    // A tag that is not a stable version asks nothing: `verify` runs.
    let (output, written, log) = verified_on("v0.3.2-beta.1", "tag", Some(1));
    assert!(output.status.success(), "{output:?}");
    assert_eq!(written, "");
    assert_eq!(log, "");
}

/// `publish` of a line's release `v0.3.2` on its tag, the tag naming `tip`, the release marked
/// latest being `latest` (none when empty, `error` for an API that fails).
fn publish_line_release(fake: &Fake, tip: &str, latest: &str) -> Output {
    fake.run(
        &step_script(".github/workflows/ci.yml", "publish"),
        &[
            ("GH_TOKEN", "unused"),
            ("CHANNEL", "stable"),
            ("VERSION", "0.3.2"),
            ("TAG", "v0.3.2"),
            ("GITHUB_REF_NAME", "v0.3.2"),
            ("GITHUB_REF_TYPE", "tag"),
            ("GITHUB_SHA", "abc123"),
            ("FAKE_TIP", tip),
            ("FAKE_LATEST", latest),
        ],
    )
}

fn created(fake: &Fake) -> String {
    let log = fake.log();
    log.lines()
        .find(|l| l.starts_with("release create"))
        .unwrap_or_else(|| panic!("{log}"))
        .to_owned()
}

#[test]
fn a_line_s_release_is_published_on_its_tag() {
    // A tag that names another commit now publishes nothing.
    let fake = Fake::new();
    let output = publish_line_release(&fake, "def456", "v0.3.1");
    assert!(output.status.success(), "{output:?}");
    assert!(!fake.log().contains("release create"), "{}", fake.log());

    let output = publish_line_release(&fake, "abc123", "v0.3.1");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fake.tags(), ["v0.3.2"]);
    assert!(fake.log().contains("api repos/o/r/commits/v0.3.2"));
    assert!(!created(&fake).contains("--prerelease"));

    // main's stable release is always marked latest.
    let fake = Fake::new();
    let output = fake.publish("stable", "main", "0.4.0");
    assert!(output.status.success(), "{output:?}");
    assert!(created(&fake).ends_with(" --latest"), "{}", created(&fake));
}

#[test]
fn a_line_s_release_above_the_latest_is_marked_latest() {
    // The stable channel reads the latest release: the newest line's fix reaches its users.
    for latest in ["v0.3.1", "v0.3.0", ""] {
        let fake = Fake::new();
        let output = publish_line_release(&fake, "abc123", latest);
        assert!(output.status.success(), "{latest}: {output:?}");
        let created = created(&fake);
        assert!(created.ends_with(" --latest"), "{latest}: {created}");
        assert!(!created.contains("--prerelease"), "{created}");
    }
}

#[test]
fn a_line_s_release_below_the_latest_is_not() {
    // An older line's: the stable channel keeps the newer release.
    for latest in ["v0.4.0", "v0.3.10", "v0.3.2"] {
        let fake = Fake::new();
        let output = publish_line_release(&fake, "abc123", latest);
        assert!(output.status.success(), "{latest}: {output:?}");
        let created = created(&fake);
        assert!(created.ends_with(" --latest=false"), "{latest}: {created}");
    }
    // Not knowing which is latest, nothing is published: run again, it is.
    let fake = Fake::new();
    let output = publish_line_release(&fake, "abc123", "error");
    assert!(!output.status.success(), "{output:?}");
    assert!(!fake.log().contains("release create"), "{}", fake.log());
}

#[test]
fn publish_never_runs_on_a_commit_no_verify_passed_on() {
    let workflow = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    // A job's lines: from its name to the next job's, the next line indented by two spaces only.
    let job = |name: &str| {
        let lines: Vec<&str> = workflow.lines().collect();
        let at = lines
            .iter()
            .position(|l| *l == format!("  {name}:"))
            .unwrap_or_else(|| panic!("a job {name}"));
        lines[at + 1..]
            .iter()
            .take_while(|l| !(l.starts_with("  ") && !l.starts_with("   ") && l.ends_with(':')))
            .copied()
            .collect::<Vec<_>>()
            .join("\n")
    };
    // `verify` is skipped only when the commit passed it on dev.
    let verify = job("verify");
    assert!(
        verify.contains(
            "needs: [verified-on-dev, changes, check, build-tests, doc-tests, test, test-selected, guards, real-browser, hub, android]"
        ),
        "{verify}"
    );
    assert!(
        verify.contains("if: ${{ always() && needs.verified-on-dev.outputs.verified != 'true' }}"),
        "{verify}"
    );
    // `publish` needs `verify` to have passed, or to have been skipped for that reason.
    let publish = job("publish");
    assert!(
        publish.contains(
            "needs: [verified-on-dev, changes, verify, build, hub, android, android-sign]"
        ),
        "{publish}"
    );
    let condition: String = publish.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        condition.contains(
            "&& (needs.verify.result == 'success' || (needs.verify.result == 'skipped' \
             && needs.verified-on-dev.outputs.verified == 'true'))"
        ),
        "{publish}"
    );
    assert!(
        condition.contains("&& needs.build.result == 'success'"),
        "{publish}"
    );
    // Only main asks dev.
    let asks = job("verified-on-dev");
    assert!(
        asks.contains(
            "if: (github.ref == 'refs/heads/main' || startsWith(github.ref, 'refs/tags/v')) &&"
        ),
        "{asks}"
    );
}

#[test]
fn verify_fails_unless_the_checks_and_every_partition_passed() {
    // `verify` is the required check: a partition that failed, was cancelled or never ran fails
    // it, as a failed `check` does.
    let script = step_script(".github/workflows/ci.yml", "verdict");
    let run = |check: &str, test: &str| {
        Fake::new()
            .run(
                &script,
                &[
                    ("CHECK", check),
                    ("BUILD", "success"),
                    ("DOC", "success"),
                    ("TEST", test),
                    ("RUST", "true"),
                ],
            )
            .status
            .success()
    };

    assert!(run("success", "success"));
    for test in ["failure", "cancelled", "skipped"] {
        assert!(!run("success", test), "test: {test}");
    }
    for check in ["failure", "cancelled"] {
        assert!(!run(check, "skipped"), "check: {check}");
    }
}

#[test]
fn verify_fails_when_the_tests_were_not_built_or_the_doc_tests_failed() {
    // The partitions are skipped when the archive was not built, and the doc tests are a job of
    // their own: neither may let a red suite through.
    let script = step_script(".github/workflows/ci.yml", "verdict");
    let run = |build: &str, doc: &str, test: &str| {
        Fake::new()
            .run(
                &script,
                &[
                    ("CHECK", "success"),
                    ("BUILD", build),
                    ("DOC", doc),
                    ("TEST", test),
                    ("RUST", "true"),
                ],
            )
            .status
            .success()
    };

    assert!(run("success", "success", "success"));
    for state in ["failure", "cancelled", "skipped"] {
        assert!(!run(state, "success", "skipped"), "build-tests: {state}");
        assert!(!run("success", state, "success"), "doc-tests: {state}");
    }
}

#[test]
fn verify_passes_skipped_jobs_only_when_the_paths_said_the_suite_is_not_needed() {
    let script = step_script(".github/workflows/ci.yml", "verdict");
    let run = |rust: &str, check: &str, test: &str| {
        Fake::new()
            .run(
                &script,
                &[
                    ("CHECK", check),
                    ("BUILD", test),
                    ("DOC", test),
                    ("TEST", test),
                    ("RUST", rust),
                ],
            )
            .status
            .success()
    };

    // The decision said "not needed": the jobs it left out are a pass.
    assert!(run("false", "skipped", "skipped"));
    // Anything else about them is not: they ran and failed, or were cancelled.
    for (check, test) in [
        ("failure", "skipped"),
        ("skipped", "failure"),
        ("cancelled", "skipped"),
        ("skipped", "cancelled"),
        ("success", "skipped"),
    ] {
        assert!(!run("false", check, test), "{check}, {test}");
    }
    // No decision (the job failed, its output is empty) or "needed": skipped is a failure.
    for rust in ["", "true"] {
        assert!(!run(rust, "skipped", "skipped"), "rust: {rust:?}");
    }
}

/// The `changes` step: whether the Rust suite must run for what `paths` changes between two
/// commits of a throwaway repository, as a pull request (or another event).
fn suite_needed(event: &str, paths: &[&str]) -> String {
    decision("decide", event, paths)
}

/// What the step `id` of `changes` writes for `paths` changed between two commits of a
/// throwaway repository, as `event`.
fn decision(id: &str, event: &str, paths: &[&str]) -> String {
    let repo = Repo::new();
    let write = |paths: &[&str]| {
        for path in paths {
            let file = repo.dir.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, format!("{path} {}", paths.len())).unwrap();
        }
        repo.git(&["add", "--all"]);
        repo.git(&["commit", "--quiet", "--allow-empty", "-m", "change"]);
    };
    write(&[
        "Cargo.lock",
        "site/README.md",
        "apps/android/README.md",
        "docs/a.md",
    ]);
    let base = repo.head();
    write(paths);
    let head = repo.head();
    let out = tempfile::tempdir().unwrap();
    let output_file = out.path().join("github-output");
    let script = step_script(".github/workflows/ci.yml", id);
    let mut command = Command::new("bash");
    command
        .args(["-e", "-c", &script])
        .current_dir(repo.dir.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("EVENT_NAME", event)
        .env("BASE", &base)
        .env("HEAD", &head)
        .env("GITHUB_OUTPUT", &output_file);
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    std::fs::read_to_string(&output_file)
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn the_rust_suite_does_not_run_for_the_android_app_or_the_site_alone() {
    for paths in [
        &["site/README.md"][..],
        &["apps/android/README.md"],
        &["site/pages/index.html", "apps/android/app/build.gradle.kts"],
    ] {
        assert_eq!(
            suite_needed("pull_request", paths),
            "rust=false",
            "{paths:?}"
        );
    }
}

#[test]
fn the_rust_suite_runs_when_anything_else_changed_too() {
    for paths in [
        &["site/README.md", "docs/a.md"][..],
        &["apps/android/README.md", "Cargo.lock"],
        &["crates/flottille/src/main.rs"],
        &[".github/workflows/ci.yml"],
        &["Cargo.lock"],
        &["docs/a.md"],
        // Only a folder of that name counts.
        &["site-map/index.html"],
        &["apps/android-tools/run.sh"],
        &["apps/ios/README.md"],
    ] {
        assert_eq!(
            suite_needed("pull_request", paths),
            "rust=true",
            "{paths:?}"
        );
    }
}

#[test]
fn the_rust_suite_always_runs_off_a_pull_request() {
    // A push publishes, main is released from it: whatever it changed, the suite ran.
    for event in ["push", "workflow_dispatch"] {
        assert_eq!(
            suite_needed(event, &["site/README.md"]),
            "rust=true",
            "{event}"
        );
    }
}

#[test]
fn the_real_browsers_job_runs_for_the_files_of_its_sandbox_and_on_every_push() {
    // The tests that drive the real agent-browser (#1395) are in a job of their own, which a pull
    // request runs when it changes what they exercise.
    for paths in [
        &["crates/flottille/src/extensions/connections/network.rs"][..],
        &["crates/flottille/src/extensions/connections/adapters.rs"],
        &["crates/flottille/src/landlock.rs"],
        &["crates/flottille/src/mounts.rs"],
        &["crates/flottille/src/browsers.rs"],
        &["crates/flottille/tests/connections.rs"],
        &[".github/workflows/ci.yml"],
        &["docs/a.md", "crates/flottille/src/landlock.rs"],
    ] {
        assert_eq!(
            decision("browser", "pull_request", paths),
            "browser=true",
            "{paths:?}"
        );
    }
    for paths in [
        &["docs/a.md"][..],
        &["site/README.md", "apps/android/README.md"],
        &["crates/flottille/src/main.rs"],
        &["crates/flottille/src/extensions/other.rs"],
    ] {
        assert_eq!(
            decision("browser", "pull_request", paths),
            "browser=false",
            "{paths:?}"
        );
    }
    // A push publishes: whatever it changed, the job ran.
    for event in ["push", "workflow_dispatch"] {
        assert_eq!(
            decision("browser", event, &["docs/a.md"]),
            "browser=true",
            "{event}"
        );
    }
}

#[test]
fn the_real_browsers_job_installs_what_its_tests_drive_and_fails_on_a_skip() {
    let job = ci_job("real-browser");
    assert!(job.contains("needs: [verified-on-dev, changes]"), "{job}");
    // `!= 'false'`: a decision that failed, and said nothing, runs the job.
    for condition in [
        "needs.verified-on-dev.outputs.verified != 'true'",
        "needs.changes.outputs.rust != 'false'",
        "needs.changes.outputs.browser != 'false'",
    ] {
        assert!(job.contains(condition), "{condition}: {job}");
    }
    // A skip is a failure in this job, and only in this one.
    assert!(job.contains("FLOTTILLE_REAL_BROWSER: required"), "{job}");
    let workflow = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    assert_eq!(workflow.matches("FLOTTILLE_REAL_BROWSER").count(), 1);
    for installed in ["passt", "weston", "FLOTTILLE_TEST_AGENT_BROWSER"] {
        assert!(job.contains(installed), "{installed}: {job}");
    }
    // The version the adapter is checked on (`range: "0.43.x"`), no other.
    let adapters = std::fs::read_to_string(tool(
        "crates/flottille/src/extensions/connections/adapters.rs",
    ))
    .unwrap();
    let range = adapters
        .lines()
        .find_map(|l| l.trim().strip_prefix("range: \""))
        .and_then(|l| l.strip_suffix("x\","))
        .expect("the adapter's range");
    assert!(
        job.contains(&format!("agent-browser@{range}0")),
        "agent-browser@{range}0: {job}"
    );
    // The tests of `connections::` in `group_remote`, none filtered out.
    assert!(job.contains("connections::"), "{job}");
    assert!(job.contains("--test group_remote"), "{job}");
}

#[test]
fn verify_fails_when_the_real_browsers_tests_failed_or_were_cancelled() {
    let script = step_script(".github/workflows/ci.yml", "verdict");
    let run = |real: &str| {
        Fake::new()
            .run(
                &script,
                &[
                    ("CHECK", "success"),
                    ("BUILD", "success"),
                    ("DOC", "success"),
                    ("TEST", "success"),
                    ("RUST", "true"),
                    ("REAL_BROWSER", real),
                ],
            )
            .status
            .success()
    };

    // Skipped is a pass: the pull request left the browser's files alone.
    for passed in ["success", "skipped"] {
        assert!(run(passed), "real-browser: {passed}");
    }
    for state in ["failure", "cancelled"] {
        assert!(!run(state), "real-browser: {state}");
    }
}

#[test]
fn the_suite_runs_only_when_the_decision_does_not_say_otherwise() {
    let workflow = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    for name in ["check", "build-tests", "doc-tests"] {
        let lines: Vec<&str> = workflow.lines().collect();
        let at = lines
            .iter()
            .position(|l| *l == format!("  {name}:"))
            .unwrap();
        let job = lines[at + 1..]
            .iter()
            .take_while(|l| !(l.starts_with("  ") && !l.starts_with("   ") && l.ends_with(':')))
            .copied()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(job.contains("needs: [verified-on-dev, changes]"), "{job}");
        // `!= 'false'`: a decision that failed, and said nothing, runs the suite.
        assert!(
            job.contains("&& needs.changes.outputs.rust != 'false'"),
            "{job}"
        );
    }
}

/// What `.github/scripts/pr-tests.sh` selects for a pull request: `base` is a throwaway
/// repository's files, `changed` are written over (or added to) them and `deleted` removed, in the
/// pull request's one commit, whose description is `body`.
fn selected_tests(base: &[&str], changed: &[&str], deleted: &[&str], body: &str) -> String {
    let output = selection(base, changed, deleted, body);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// The whole output of `.github/scripts/pr-tests.sh` (its notices and warnings are on stderr).
fn selection(
    base: &[&str],
    changed: &[&str],
    deleted: &[&str],
    body: &str,
) -> std::process::Output {
    let repo = Repo::new();
    // A file is `path`, or `path::text` when the text it holds matters.
    let write = |paths: &[&str], content: &str| {
        for path in paths {
            let (path, text) = path.split_once("::").unwrap_or((path, content));
            let file = repo.dir.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, text).unwrap();
        }
    };
    write(base, "base");
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "--quiet", "--allow-empty", "-m", "base"]);
    let base_commit = repo.head();
    write(changed, "changed");
    for path in deleted {
        std::fs::remove_file(repo.dir.path().join(path)).unwrap();
    }
    repo.git(&["add", "--all"]);
    repo.git(&["commit", "--quiet", "--allow-empty", "-m", "change"]);
    let output = Command::new("bash")
        .arg(tool(".github/scripts/pr-tests.sh"))
        .current_dir(repo.dir.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("BASE", &base_commit)
        .env("HEAD", repo.head())
        .env("BODY", body)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    output
}

#[test]
fn a_pull_request_runs_the_test_binaries_whose_files_it_changes() {
    let base = ["crates/flottille-core/tests/families.rs"];
    assert_eq!(
        selected_tests(&base, &["crates/flottille-core/tests/families.rs"], &[], ""),
        "--test families"
    );
    // Every crate's tests/ folder, the binaries in alphabetical order, each once.
    assert_eq!(
        selected_tests(
            &base,
            &[
                "crates/flottille/tests/release.rs",
                "crates/flottille-core/tests/families.rs",
                "crates/flottille/tests/alerts.rs",
                "crates/flottille-core/src/lib.rs",
            ],
            &[],
            ""
        ),
        "--test alerts --test families --test release"
    );
}

#[test]
fn a_pull_request_runs_a_grouped_binary_for_any_file_of_its_folder() {
    // A binary made of a folder (tests/<name>/main.rs, the way #1106 groups the files) is run for
    // a change of any file in it; a folder with no main.rs (shared helpers) is no binary.
    let base = [
        "crates/flottille/tests/sessions/main.rs",
        "crates/flottille/tests/common/mod.rs",
    ];
    assert_eq!(
        selected_tests(&base, &["crates/flottille/tests/sessions/wake.rs"], &[], ""),
        "--test sessions"
    );
    assert_eq!(
        selected_tests(&base, &["crates/flottille/tests/common/mod.rs"], &[], ""),
        ""
    );
}

/// The files of a crate with grouped test binaries: `Cargo.toml` names the binaries, each main of
/// `tests/group/` declares the files it holds (`#1106`).
fn grouped_crate() -> Vec<&'static str> {
    vec![
        "crates/flottille/Cargo.toml::[package]\nname = \"flottille\"\n\n[[test]]\nname = \"group_window_session\"\npath = \"tests/group/window_session.rs\"\n\n[[test]]\nname = \"group_agents\"\npath = \"tests/group/agents.rs\"\n",
        "crates/flottille/tests/group/window_session.rs::#[path = \"../common/mod.rs\"]\nmod common;\n#[path = \"../window_conversation.rs\"]\nmod window_conversation;\n#[path = \"../window_cards.rs\"]\nmod window_cards;\n",
        "crates/flottille/tests/group/agents.rs::#[path = \"../common/mod.rs\"]\nmod common;\n#[path = \"../agents.rs\"]\nmod agents;\n",
        "crates/flottille/tests/common/mod.rs",
        "crates/flottille/tests/window_conversation.rs",
        "crates/flottille/tests/window_cards.rs",
        "crates/flottille/tests/agents.rs",
        "crates/flottille/tests/orphan.rs",
        "crates/flottille-core/tests/families.rs",
    ]
}

#[test]
fn a_pull_request_runs_only_the_file_it_changes_of_a_grouped_binary() {
    let base = grouped_crate();
    assert_eq!(
        selected_tests(
            &base,
            &["crates/flottille/tests/window_conversation.rs"],
            &[],
            ""
        ),
        "--test group_window_session -E binary(=group_window_session)&(test(/^window_conversation::/))"
    );
    // Two files of one binary share its filter; a file of another binary has its own, and a binary
    // of another crate, run whole, is in the same expression.
    assert_eq!(
        selected_tests(
            &base,
            &[
                "crates/flottille/tests/window_conversation.rs",
                "crates/flottille/tests/agents.rs",
                "crates/flottille/tests/window_cards.rs",
                "crates/flottille-core/tests/families.rs",
            ],
            &[],
            ""
        ),
        "--test families --test group_agents --test group_window_session -E \
binary(=group_agents)&(test(/^agents::/))|\
binary(=group_window_session)&(test(/^window_cards::/)|test(/^window_conversation::/))|\
binary(=families)"
    );
}

#[test]
fn a_pull_request_that_changes_the_run_sandbox_runs_the_real_programs_test() {
    // The other tests drive the sandbox with fakes, which read nothing as a real program does
    // (#1303): a change of the sandbox, of the run tool or of what makes the system's files
    // readable in it runs the test of real programs, and only that file of its group.
    let mut base = grouped_crate();
    base.push("crates/flottille/tests/real_programs.rs");
    base.push("crates/flottille/src/mounts.rs");
    base.push("crates/flottille/src/landlock.rs");
    base.push("crates/flottille/src/ssh_owned.rs");
    base.push("crates/flottille/src/runs.rs");
    base.push("crates/flottille/src/gate.rs");
    base[2] = "crates/flottille/tests/group/agents.rs::#[path = \"../real_programs.rs\"]\nmod real_programs;\n";
    for sandbox in ["mounts", "landlock", "ssh_owned", "runs"] {
        assert_eq!(
            selected_tests(
                &base,
                &[&format!("crates/flottille/src/{sandbox}.rs")],
                &[],
                ""
            ),
            "--test group_agents -E binary(=group_agents)&(test(/^real_programs::/))",
            "{sandbox}"
        );
    }
    // Another file of the crate's sources selects nothing of it.
    assert_eq!(
        selected_tests(&base, &["crates/flottille/src/gate.rs"], &[], ""),
        ""
    );
}

#[test]
fn the_jobs_that_run_tests_have_the_programs_the_real_programs_test_runs() {
    // `ubuntu-latest` has ssh, git, gh, curl, node and cargo; pnpm is the one it lacks. The test
    // fails where `CI` is set and a program is missing, so that none is skipped there.
    for name in ["test-selected", "test"] {
        let job = ci_job(name);
        assert!(job.contains("npm install --global pnpm"), "{name}: {job}");
    }
}

/// The `#[path = "../<name>.rs"]` files each group main of `tests/group/` declares: `(group, file)`.
fn declared_by_the_groups() -> Vec<(String, String)> {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut declared = Vec::new();
    for entry in std::fs::read_dir(tests.join("group")).unwrap() {
        let main = entry.unwrap().path();
        let group = main.file_stem().unwrap().to_string_lossy().into_owned();
        for line in std::fs::read_to_string(&main).unwrap().lines() {
            if let Some(path) = line
                .strip_prefix("#[path = \"../")
                .and_then(|rest| rest.strip_suffix("\"]"))
            {
                declared.push((group.clone(), path.to_owned()));
            }
        }
    }
    declared
}

#[test]
fn every_test_file_is_declared_by_exactly_one_group() {
    // `autotests = false`: a file no group declares is never built nor run, and nothing says so.
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let declared = declared_by_the_groups();
    let mut missing = Vec::new();
    let mut twice = Vec::new();
    for entry in std::fs::read_dir(&tests).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let groups: Vec<&str> = declared
            .iter()
            .filter(|(_, declared)| *declared == file)
            .map(|(group, _)| group.as_str())
            .collect();
        let stem = file.trim_end_matches(".rs");
        match groups.len() {
            0 => missing.push(format!(
                "tests/{file} is declared by no group, so it is never built nor run: add \
                 `#[path = \"../{file}\"]` and `mod {stem};` to the group in tests/group/ that fits \
                 (AGENTS.md, \"Adding a test file\")"
            )),
            1 => {}
            _ => twice.push(format!(
                "tests/{file} is declared by {} groups ({}): keep one",
                groups.len(),
                groups.join(", ")
            )),
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
    assert!(twice.is_empty(), "{}", twice.join("\n"));
}

#[test]
fn every_path_a_group_declares_is_a_file_that_exists() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let gone: Vec<String> = declared_by_the_groups()
        .into_iter()
        .filter(|(_, file)| !tests.join("group").join("..").join(file).exists())
        .map(|(group, file)| {
            format!("tests/group/{group}.rs declares ../{file}, which does not exist")
        })
        .collect();
    assert!(gone.is_empty(), "{}", gone.join("\n"));
}

#[test]
fn a_file_a_group_does_not_declare_selects_no_test() {
    let base = grouped_crate();
    assert_eq!(
        selected_tests(&base, &["crates/flottille/tests/orphan.rs"], &[], ""),
        ""
    );
    // The helpers every group shares are no test either: a change of them selects nothing (the
    // description says what to run, `Tests:`).
    assert_eq!(
        selected_tests(&base, &["crates/flottille/tests/common/mod.rs"], &[], ""),
        ""
    );
}

#[test]
fn a_changed_test_file_no_group_declares_is_a_warning_not_a_notice() {
    let output = selection(
        &grouped_crate(),
        &["crates/flottille/tests/orphan.rs"],
        &[],
        "",
    );
    let said = String::from_utf8(output.stderr).unwrap();
    assert!(
        said.contains("::warning::crates/flottille/tests/orphan.rs is declared by no group"),
        "{said}"
    );
    assert!(said.contains("#[path = \"../orphan.rs\"]"), "{said}");
}

#[test]
fn a_file_a_group_gains_is_run_and_the_rest_of_its_main_is_not() {
    let base = grouped_crate();
    // The main of the group now declares one more file: that file's tests run, not the group's.
    let main = "crates/flottille/tests/group/agents.rs::#[path = \"../common/mod.rs\"]\nmod common;\n#[path = \"../agents.rs\"]\nmod agents;\n#[path = \"../orphan.rs\"]\nmod orphan;\n";
    assert_eq!(
        selected_tests(&base, &[main, "crates/flottille/tests/orphan.rs"], &[], ""),
        "--test group_agents -E binary(=group_agents)&(test(/^orphan::/))"
    );
    // The main changed and no file with it, a file moved there from another group.
    assert_eq!(
        selected_tests(&base, &[main], &[], ""),
        "--test group_agents -E binary(=group_agents)&(test(/^orphan::/))"
    );
    // A main that only lost a line selects nothing.
    let less = "crates/flottille/tests/group/window_session.rs::#[path = \"../common/mod.rs\"]\nmod common;\n#[path = \"../window_cards.rs\"]\nmod window_cards;\n";
    assert_eq!(selected_tests(&base, &[less], &[], ""), "");
}

#[test]
fn the_tests_line_names_a_grouped_file_or_a_group() {
    let base = grouped_crate();
    // A file's name, as the issues and the descriptions of before the groups say it.
    assert_eq!(
        selected_tests(&base, &["docs/a.md"], &[], "Tests: window_conversation"),
        "--test group_window_session -E binary(=group_window_session)&(test(/^window_conversation::/))"
    );
    // A group's name runs the whole binary, and a file of it adds nothing to it.
    assert_eq!(
        selected_tests(
            &base,
            &["crates/flottille/tests/window_cards.rs"],
            &[],
            "Tests: group_window_session"
        ),
        "--test group_window_session -E binary(=group_window_session)"
    );
}

#[test]
fn a_test_file_added_is_run_and_one_deleted_is_not() {
    let base = [
        "crates/flottille/tests/old.rs",
        "crates/flottille/tests/kept.rs",
    ];
    assert_eq!(
        selected_tests(
            &base,
            &["crates/flottille/tests/new.rs"],
            &["crates/flottille/tests/old.rs"],
            ""
        ),
        "--test new"
    );
}

#[test]
fn the_tests_line_of_the_description_adds_the_binaries_it_lists() {
    let base = ["crates/flottille/tests/other.rs"];
    let changed = ["crates/flottille/tests/other.rs"];
    for (body, expected) in [
        ("Tests: families", "--test other --test families"),
        (
            "Closes #1\r\n\r\nTests: families, release\r\n",
            "--test other --test families --test release",
        ),
        (
            "text\n  Tests:  families   release  \nmore",
            "--test other --test families --test release",
        ),
        // A binary a file already selected is run once.
        ("Tests: other", "--test other"),
        // Another line, or the word in a sentence, is no list.
        ("The Tests: families are slow", "--test other"),
        ("Test: families", "--test other"),
    ] {
        assert_eq!(
            selected_tests(&base, &changed, &[], body),
            expected,
            "{body:?}"
        );
    }
    // Alone: nothing else changed.
    assert_eq!(
        selected_tests(&base, &["docs/a.md"], &[], "Tests: families"),
        "--test families"
    );
}

#[test]
fn a_name_that_is_not_a_binary_name_is_never_passed_on() {
    // The description is the author's, and its words reach a command line: only letters, digits,
    // dashes and underscores are names.
    let output = selected_tests(
        &[],
        &["docs/a.md"],
        &[],
        "Tests: ok $(touch pwned) a;b `id` --release ../x ok-2_b",
    );
    assert_eq!(output, "--test ok --test ok-2_b");
}

#[test]
fn the_unit_tests_of_a_small_crate_run_when_no_binary_is_selected() {
    let base = [
        "crates/flottille-core/src/lib.rs",
        "crates/flottille-protocol/src/lib.rs",
    ];
    assert_eq!(
        selected_tests(&base, &["crates/flottille-core/src/lib.rs"], &[], ""),
        "-p flottille-core --lib"
    );
    assert_eq!(
        selected_tests(
            &base,
            &[
                "crates/flottille-protocol/src/lib.rs",
                "crates/flottille-core/src/model/mod.rs",
            ],
            &[],
            ""
        ),
        "-p flottille-core -p flottille-protocol --lib"
    );
}

#[test]
fn the_unit_tests_do_not_run_beside_a_binary_nor_for_a_big_crate_nor_for_a_crate_with_no_library() {
    let base = [
        "crates/flottille-core/src/lib.rs",
        "crates/flottille/src/lib.rs",
        "crates/flottille-window/src/lib.rs",
        "crates/flottille-fake-gh/src/main.rs",
    ];
    // A binary is selected (by a file, or by the description): the crates' unit tests are not.
    assert_eq!(
        selected_tests(
            &base,
            &[
                "crates/flottille-core/src/lib.rs",
                "crates/flottille-core/tests/families.rs"
            ],
            &[],
            ""
        ),
        "--test families"
    );
    assert_eq!(
        selected_tests(
            &base,
            &["crates/flottille-core/src/lib.rs"],
            &[],
            "Tests: release"
        ),
        "--test release"
    );
    // flottille and the window are too big to build for their unit tests alone; a fake has no
    // library.
    for path in [
        "crates/flottille/src/lib.rs",
        "crates/flottille-window/src/lib.rs",
        "crates/flottille-fake-gh/src/main.rs",
    ] {
        assert_eq!(selected_tests(&base, &[path], &[], ""), "", "{path}");
    }
}

#[test]
fn a_changed_workflow_or_tool_runs_the_tests_that_name_it() {
    // The tests that read ci.yml or run package.sh are the ones to judge a change of them.
    let base = [
        "crates/flottille/tests/release.rs::reads .github/workflows/ci.yml and tools/package.sh",
        "crates/flottille-hub/tests/publish.rs::reads .github/workflows/ci.yml",
        "crates/flottille/tests/grouped/main.rs::runs tools/package.sh",
        "crates/flottille/tests/other.rs::says nothing of them",
    ];
    assert_eq!(
        selected_tests(&base, &[".github/workflows/ci.yml"], &[], ""),
        "--test publish --test release"
    );
    assert_eq!(
        selected_tests(&base, &["tools/package.sh"], &[], ""),
        "--test grouped --test release"
    );
    // No test names it, and a file of the docs is no workflow: nothing is selected.
    assert_eq!(
        selected_tests(&base, &[".github/scripts/none.sh"], &[], ""),
        ""
    );
    assert_eq!(selected_tests(&base, &["docs/a.md"], &[], ""), "");
}

#[test]
fn a_pull_request_that_changes_no_test_and_no_source_runs_no_test() {
    assert_eq!(
        selected_tests(&["docs/a.md"], &["docs/a.md", "Cargo.lock"], &[], ""),
        ""
    );
}

#[test]
fn a_pull_request_runs_the_selected_tests_alone_and_a_push_the_whole_suite() {
    let selected = ci_job("test-selected");
    assert!(
        selected.contains("needs: [verified-on-dev, changes]"),
        "{selected}"
    );
    let condition: String = selected.split_whitespace().collect::<Vec<_>>().join(" ");
    for part in [
        "needs.changes.outputs.rust != 'false'",
        "needs.changes.outputs.suite == 'false'",
        "needs.changes.outputs.selected != ''",
    ] {
        assert!(condition.contains(part), "{part}: {selected}");
    }
    // The whole suite: unless the pull request said it runs the fast check (`suite=false`, which
    // only the step that read the pull request writes: a failure of it runs everything).
    for name in ["build-tests", "doc-tests"] {
        let job = ci_job(name);
        assert!(
            job.contains("&& needs.changes.outputs.suite != 'false'"),
            "{name}: {job}"
        );
    }
    let test = ci_job("test");
    assert!(test.contains("needs: [build-tests]"), "{test}");
    // `verify` waits for the selected tests, and reads their flaky ones.
    let verify = ci_job("verify");
    assert!(
        verify.contains(
            "needs: [verified-on-dev, changes, check, build-tests, doc-tests, test, test-selected, guards, real-browser, hub, android]"
        ),
        "{verify}"
    );
    // A red Android job (the phone's core on an emulator) is never merged; skipped is a pass.
    assert!(
        verify.contains("ANDROID: ${{ needs.android.result }}")
            && verify
                .contains(r#"if [ "$ANDROID" = failure ] || [ "$ANDROID" = cancelled ]; then"#),
        "{verify}"
    );
}

#[test]
fn a_partition_makes_the_scratch_folder_cargo_makes_before_it_runs_the_tests() {
    // The tests keep scratch files in `CARGO_TARGET_TMPDIR` (`target/tmp`), which cargo makes when
    // it runs a test and an archive does not hold: run 44623882394 failed on it, 27 tests.
    let test = ci_job("test");
    let make = test
        .find("mkdir -p \"$GITHUB_WORKSPACE/target/tmp\"")
        .expect("the folder is made");
    let run = test
        .find("cargo nextest run --archive-file")
        .expect("the run");
    assert!(make < run, "{test}");
}

#[test]
fn every_job_that_compiles_links_with_mold() {
    for name in [
        "check",
        "build-tests",
        "test-selected",
        "guards",
        "doc-tests",
        "build",
        "hub",
    ] {
        let job = ci_job(name);
        assert!(job.contains("uses: rui314/setup-mold@v1"), "{name}: {job}");
    }
}

#[test]
fn a_pull_request_passes_verify_on_its_checks_and_its_selected_tests() {
    let script = step_script(".github/workflows/ci.yml", "verdict");
    let run = |suite: &str, check: &str, selected: &str| {
        Fake::new()
            .run(
                &script,
                &[
                    ("CHECK", check),
                    ("BUILD", "skipped"),
                    ("DOC", "skipped"),
                    ("TEST", "skipped"),
                    ("SELECTED", selected),
                    ("GUARDS", "success"),
                    ("SUITE", suite),
                    ("RUST", "true"),
                ],
            )
            .status
            .success()
    };

    // The selected tests ran and passed, or there were none to run.
    assert!(run("false", "success", "success"));
    assert!(run("false", "success", "skipped"));
    // Anything else is red, as ever.
    for selected in ["failure", "cancelled"] {
        assert!(!run("false", "success", selected), "selected: {selected}");
    }
    for check in ["failure", "cancelled", "skipped"] {
        assert!(!run("false", check, "success"), "check: {check}");
    }
    // The decision that failed (no `suite`) or says "whole": the selected tests are not the suite.
    for suite in ["", "true"] {
        assert!(!run(suite, "success", "success"), "suite: {suite:?}");
    }
}

#[test]
fn a_pull_request_runs_the_guards_whatever_it_changes() {
    // The tests that hold a rule over the whole repository (every command has its section of the
    // docs and its place in the window, every gap is a step of the scale, every child is spawned
    // by the daemon) failed dev eleven times in two days (21-23 November 2026): no pull request
    // selected them, as none changed their files. They run on every pull request the Rust suite
    // runs for, beside the selected tests, whatever it selects.
    let guards = ci_job("guards");
    assert!(
        guards.contains("needs: [verified-on-dev, changes]"),
        "{guards}"
    );
    let condition = condition_of(&guards);
    let context = |rust: &'static str, suite: &'static str| {
        [
            ("needs.verified-on-dev.outputs.verified", ""),
            ("needs.changes.outputs.rust", rust),
            ("needs.changes.outputs.suite", suite),
        ]
    };
    assert!(holds(&condition, &context("true", "false")));
    // The whole suite holds them already; a change of the Android app or the site alone does not
    // build the Rust code.
    assert!(!holds(&condition, &context("true", "")));
    assert!(!holds(&condition, &context("false", "false")));
    for guard in [
        "--bin flottille",
        "--test window_rules",
        "--test group_daemon",
        "test(/^window_parity::/)",
    ] {
        assert!(guards.contains(guard), "{guard}: {guards}");
    }
    assert!(guards.contains("--profile ci"), "{guards}");
}

#[test]
fn verify_fails_a_pull_request_whose_guards_did_not_pass() {
    let script = step_script(".github/workflows/ci.yml", "verdict");
    let run = |guards: &str| {
        Fake::new()
            .run(
                &script,
                &[
                    ("CHECK", "success"),
                    ("BUILD", "skipped"),
                    ("DOC", "skipped"),
                    ("TEST", "skipped"),
                    ("SELECTED", "success"),
                    ("GUARDS", guards),
                    ("SUITE", "false"),
                    ("RUST", "true"),
                ],
            )
            .status
            .success()
    };

    assert!(run("success"));
    for guards in ["failure", "cancelled", "skipped"] {
        assert!(!run(guards), "guards: {guards}");
    }
}

#[test]
fn the_ci_names_an_alpha_like_a_beta() {
    let script = step_script(".github/workflows/ci.yml", "version");
    assert!(!script.contains("alpha-$"), "{script}");
    assert!(script.contains("tag=\"v$version\""), "{script}");
}

#[test]
fn a_deleted_branch_takes_every_alpha_with_it_and_no_other() {
    let fake = Fake::new();
    fake.state(&[
        "v0.2.0-alpha.feat-a.1",
        "v0.2.0-alpha.feat-a.12",
        "draft:v0.2.1-alpha.feat-a.13",
        // Another branch, a longer name, a beta and a stable one: kept.
        "v0.2.0-alpha.feat-a-b.3",
        "v0.2.0-alpha.other.1",
        "v0.2.0-beta.4",
        "v0.2.0",
        // The rolling release of the old design.
        "alpha-feat-a",
        "alpha-feat-a-b",
    ]);

    let output = fake.run(
        &step_script(".github/workflows/alpha-cleanup.yml", "forget"),
        &[("GH_TOKEN", "unused"), ("BRANCH", "feat/a")],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fake.tags(),
        [
            "v0.2.0-alpha.feat-a-b.3",
            "v0.2.0-alpha.other.1",
            "v0.2.0-beta.4",
            "v0.2.0",
            "alpha-feat-a-b",
        ]
    );
    // The tags go with their releases.
    for line in fake
        .log()
        .lines()
        .filter(|l| l.starts_with("release delete"))
    {
        assert!(line.contains("--cleanup-tag"), "{line}");
    }
}

#[test]
fn a_deleted_branch_with_no_alpha_is_not_an_error() {
    let fake = Fake::new();
    fake.state(&["v0.2.0"]);

    let output = fake.run(
        &step_script(".github/workflows/alpha-cleanup.yml", "forget"),
        &[("GH_TOKEN", "unused"), ("BRANCH", "feat/none")],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(fake.tags(), ["v0.2.0"]);
}

#[test]
fn the_version_says_the_channel_and_the_commit() {
    let output = Command::new(env!("CARGO_BIN_EXE_flottille"))
        .arg("--version")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&output.stdout);
    // A build CI did not make is a local one.
    let channel = option_env!("FLOTTILLE_BUILD_CHANNEL").unwrap_or("local");
    assert!(
        said.trim().starts_with("flottille ") && said.contains(&format!("({channel}")),
        "{said}"
    );
}

/// A job of `ci.yml`: its lines, from its name to the next job's.
fn ci_job(name: &str) -> String {
    let workflow = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    let lines: Vec<&str> = workflow.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == format!("  {name}:"))
        .unwrap_or_else(|| panic!("a job {name}"));
    lines[at + 1..]
        .iter()
        .take_while(|l| !(l.starts_with("  ") && !l.starts_with("   ") && l.ends_with(':')))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_push_to_a_feature_branch_runs_no_check_and_no_test() {
    // Its pull request's run judges it (and publishes its alpha): a push run too would run the
    // whole suite twice on the same commit.
    let workflow = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    let triggers = &workflow[..workflow.find("\nconcurrency:").expect("concurrency")];
    let branches = triggers
        .lines()
        .find(|l| l.trim_start().starts_with("branches:"))
        .expect("the branches pushed");
    for kept in ["main", "dev", "stable/*", "next/*"] {
        assert!(branches.contains(kept), "{kept}: {branches}");
    }
    for feature in ["feat/", "fix/"] {
        assert!(!branches.contains(feature), "{feature}: {branches}");
    }
    assert!(triggers.contains("\n  pull_request:\n"), "{triggers}");
}

/// The `changes` step that says whether a pull request publishes an alpha, as GitHub runs it.
fn publishes_an_alpha(event: &str, head_ref: &str, head_repo: &str) -> String {
    let out = tempfile::tempdir().unwrap();
    let output_file = out.path().join("github-output");
    let output = Command::new("bash")
        .args([
            "-e",
            "-c",
            &step_script(".github/workflows/ci.yml", "alpha"),
        ])
        .env("EVENT_NAME", event)
        .env("HEAD_REF", head_ref)
        .env("HEAD_REPO", head_repo)
        .env("GITHUB_REPOSITORY", "o/r")
        .env("GITHUB_OUTPUT", &output_file)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    std::fs::read_to_string(&output_file)
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn a_pull_request_of_a_feature_branch_of_the_repository_publishes_its_alpha() {
    for head_ref in ["feat/12-topic", "fix/7-bug", "feat/a/b"] {
        assert_eq!(
            publishes_an_alpha("pull_request", head_ref, "o/r"),
            "alpha=true",
            "{head_ref}"
        );
    }
}

#[test]
fn no_other_run_publishes_an_alpha() {
    // Another branch never had one; a fork's token cannot write; a push's own build publishes its
    // channel's release, not an alpha.
    for (event, head_ref, head_repo) in [
        ("pull_request", "ci/1-topic", "o/r"),
        ("pull_request", "docs/1-topic", "o/r"),
        ("pull_request", "dev", "o/r"),
        ("pull_request", "feat/12-topic", "someone/r"),
        ("push", "", ""),
        ("workflow_dispatch", "", ""),
    ] {
        assert_eq!(
            publishes_an_alpha(event, head_ref, head_repo),
            "alpha=false",
            "{event} {head_ref} {head_repo}"
        );
    }
}

#[test]
fn the_alpha_of_a_pull_request_is_built_and_published_for_the_head_of_its_branch() {
    let build = ci_job("build");
    assert!(build.contains("needs: [changes]"), "{build}");
    let condition: String = build.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        condition.contains("|| needs.changes.outputs.alpha == 'true')"),
        "{build}"
    );
    // What is built is the head a person pushed, not the merge commit GitHub makes.
    assert!(
        build.contains("ref: ${{ github.event.pull_request.head.sha }}"),
        "{build}"
    );
    // `publish` still needs `verify` to have passed on this very run.
    let publish = ci_job("publish");
    let condition: String = publish.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        condition.contains("|| needs.changes.outputs.alpha == 'true')"),
        "{publish}"
    );
    assert!(
        condition.contains("(needs.verify.result == 'success' ||"),
        "{publish}"
    );
    // The branch's own name keeps the alpha's series and its cleanup, not `<n>/merge`.
    assert!(
        publish.contains("group: publish-${{ github.head_ref || github.ref_name }}"),
        "{publish}"
    );
}

#[test]
fn an_alpha_built_from_a_pull_request_is_named_after_its_branch_and_its_head() {
    let repo = ready_for_a_release();
    repo.git(&["checkout", "--quiet", "-b", "feat/12-topic"]);
    repo.commit("fix(cli): a typo (#2)");
    let head = repo.head();
    repo.git(&["checkout", "--quiet", "dev"]);

    // A pull request's run is on a merge ref, `12/merge`, at a commit no one pushed.
    let (output, outputs, _) = version_step_with(
        &repo,
        "12/merge",
        "branch",
        "0000000000000000000000000000000000000000",
        &[("HEAD_REF", "feat/12-topic"), ("HEAD_SHA", &head)],
    );

    assert!(output.status.success(), "{output:?}");
    assert!(outputs.starts_with("channel=alpha\nversion="), "{outputs}");
    assert!(outputs.contains("-alpha.feat-12-topic."), "{outputs}");
}

#[test]
fn the_alpha_of_a_pull_request_is_published_for_its_head_when_the_branch_still_is_there() {
    let fake = Fake::new();
    let run = |tip: &str| {
        fake.run(
            &step_script(".github/workflows/ci.yml", "publish"),
            &[
                ("GH_TOKEN", "unused"),
                ("CHANNEL", "alpha"),
                ("VERSION", "0.2.0-alpha.feat-12-topic.1"),
                ("TAG", "v0.2.0-alpha.feat-12-topic.1"),
                ("GITHUB_REF_NAME", "12/merge"),
                ("GITHUB_SHA", "merge999"),
                ("HEAD_REF", "feat/12-topic"),
                ("HEAD_SHA", "abc123"),
                ("FAKE_TIP", tip),
            ],
        )
    };

    // The branch moved on: its later push has a pull request run of its own, which publishes.
    let output = run("def456");
    assert!(output.status.success(), "{output:?}");
    assert!(!fake.log().contains("release create"), "{}", fake.log());

    let output = run("abc123");
    assert!(output.status.success(), "{output:?}");
    let log = fake.log();
    assert!(log.contains("--target abc123"), "{log}");
    assert!(!log.contains("merge999"), "{log}");
    assert!(log.contains("--prerelease --latest=false"), "{log}");
    assert!(log.contains("git/ref/heads/feat/12-topic"), "{log}");
}

#[test]
fn the_tests_are_built_once_and_run_on_twelve_partitions_of_the_archive() {
    let build = ci_job("build-tests");
    assert_eq!(build.matches("cargo nextest archive").count(), 1, "{build}");
    assert!(build.contains("--archive-file"), "{build}");
    assert!(build.contains("actions/upload-artifact"), "{build}");

    let test = ci_job("test");
    assert!(test.contains("needs: [build-tests]"), "{test}");
    assert!(test.contains("actions/download-artifact"), "{test}");
    assert!(test.contains("--archive-file"), "{test}");
    assert!(test.contains("--workspace-remap ."), "{test}");
    // The test binaries hold the paths of their build (`env!("CARGO_BIN_EXE_flottille")`, the
    // fakes beside it): the archive is extracted where `target/` was, in the checkout.
    assert!(
        test.contains("--extract-to \"$GITHUB_WORKSPACE\" --extract-overwrite"),
        "{test}"
    );
    assert!(
        test.contains("--partition count:${{ matrix.partition }}/12"),
        "{test}"
    );
    let matrix = test
        .lines()
        .find(|l| l.trim_start().starts_with("partition:"))
        .expect("the partitions");
    assert_eq!(matrix.matches(',').count() + 1, 12, "{matrix}");
    // A partition only runs: it compiles nothing, and has no cache of a build to restore.
    for building in ["cargo build", "cargo test", "cargo clippy", "rust-cache"] {
        assert!(!test.contains(building), "{building}: {test}");
    }
    // The doc tests run once, in a job of their own.
    let doc = ci_job("doc-tests");
    assert!(doc.contains("cargo test --doc --locked"), "{doc}");
    assert!(!test.contains("--doc"), "{test}");
}

#[test]
fn a_failed_test_is_retried_twice_in_the_profile_ci_uses() {
    let config = std::fs::read_to_string(tool(".config/nextest.toml")).unwrap();
    let profile = &config[config.find("[profile.ci]").expect("a profile ci")..];
    let profile = &profile[..profile[1..].find("\n[").map_or(profile.len(), |at| at + 1)];
    assert!(profile.contains("retries = 2"), "{config}");
    // A report to tell the flaky tests by: nextest writes a JUnit file for the profile.
    assert!(config.contains("[profile.ci.junit]"), "{config}");
    assert!(
        test_job_runs_profile_ci(),
        "the test job runs `--profile ci`"
    );
}

fn test_job_runs_profile_ci() -> bool {
    ci_job("test").contains("--profile ci")
}

/// The flaky tests of JUnit files, as `flaky-report.sh` lists them.
fn flaky_report(reports: &[&str]) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<PathBuf> = reports
        .iter()
        .enumerate()
        .map(|(n, xml)| {
            let file = dir.path().join(format!("junit-{n}.xml"));
            std::fs::write(&file, xml).unwrap();
            file
        })
        .collect();
    let output = Command::new(tool(".github/scripts/flaky-report.sh"))
        .args(&files)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

const JUNIT_WITH_A_FLAKY_TEST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="3" failures="1">
  <testsuite name="flottille-core::probe" tests="3">
    <testcase name="flaky_test" classname="flottille-core::probe">
      <flakyFailure type="test failure with exit code 101"><system-out>first</system-out></flakyFailure>
      <flakyFailure type="test failure with exit code 101"><system-out>second</system-out></flakyFailure>
    </testcase>
    <testcase name="steady_test" classname="flottille-core::probe"/>
    <testcase name="broken_test" classname="flottille-core::probe">
      <failure type="test failure with exit code 101"/>
      <rerunFailure type="test failure with exit code 101"/>
      <rerunFailure type="test failure with exit code 101"/>
    </testcase>
  </testsuite>
</testsuites>
"#;

#[test]
fn a_flaky_test_is_listed_with_its_attempts_under_a_heading() {
    let (ok, report) = flaky_report(&[JUNIT_WITH_A_FLAKY_TEST]);
    assert!(ok, "{report}");
    assert!(report.starts_with("## Flaky tests\n"), "{report}");
    assert!(
        report.contains("| `flottille-core::probe flaky_test` | 3 |"),
        "{report}"
    );
    // A test that passed first time, and one that failed every attempt (it fails its job), are
    // not flaky.
    assert!(!report.contains("steady_test"), "{report}");
    assert!(!report.contains("broken_test"), "{report}");
}

#[test]
fn nothing_is_said_when_no_test_was_flaky() {
    let report = r#"<?xml version="1.0"?><testsuites><testsuite name="a"><testcase name="t" classname="c"/></testsuite></testsuites>"#;
    let (ok, said) = flaky_report(&[report, report]);
    assert!(ok, "{said}");
    assert_eq!(said, "");
}

#[test]
fn the_flaky_tests_of_every_partition_are_listed_once_in_one_report() {
    let other = JUNIT_WITH_A_FLAKY_TEST
        .replace("flaky_test", "another_flaky_test")
        .replace("<flakyFailure type=\"test failure with exit code 101\"><system-out>second</system-out></flakyFailure>\n", "");
    let (ok, report) = flaky_report(&[JUNIT_WITH_A_FLAKY_TEST, &other]);
    assert!(ok, "{report}");
    assert_eq!(report.matches("## Flaky tests").count(), 1, "{report}");
    assert!(
        report.contains("`flottille-core::probe flaky_test` | 3 |"),
        "{report}"
    );
    assert!(report.contains("another_flaky_test` | 2 |"), "{report}");
}

#[test]
fn verify_and_the_partitions_both_put_the_flaky_tests_in_their_summary() {
    for job in ["test", "verify"] {
        let text = ci_job(job);
        assert!(
            text.contains(".github/scripts/flaky-report.sh"),
            "{job}: {text}"
        );
        assert!(text.contains("$GITHUB_STEP_SUMMARY"), "{job}: {text}");
    }
    // The summary is informational: it never decides the verdict.
    assert!(
        ci_job("verify").contains("continue-on-error: true"),
        "{}",
        ci_job("verify")
    );
}

// The release's APK, the Android tests offline and the betas' clean-up (#1196).

/// The `if:` of the first job or step in `text`, the way GitHub reads it: on one line, or folded
/// (`>-`) over the lines indented under it.
fn condition_of(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let at = lines
        .iter()
        .position(|l| l.trim_start().starts_with("if:"))
        .unwrap_or_else(|| panic!("no condition in {text}"));
    let indent = lines[at].len() - lines[at].trim_start().len();
    let first = lines[at].trim_start()["if:".len()..].trim();
    let mut expression = if first == ">-" {
        String::new()
    } else {
        first.to_owned()
    };
    for line in &lines[at + 1..] {
        if line.trim().is_empty() || line.len() - line.trim_start().len() <= indent {
            break;
        }
        expression.push(' ');
        expression.push_str(line.trim());
    }
    expression
        .trim()
        .trim_start_matches("${{")
        .trim_end_matches("}}")
        .trim()
        .to_owned()
}

/// Whether `expression` holds for `context`, with the operators `ci.yml` uses: `!`, `==`, `!=`,
/// `&&`, `||`, parentheses, quoted strings, `cancelled()` and `always()`. A name the context does
/// not give fails the test: a condition that reads something new says so here.
fn holds(expression: &str, context: &[(&str, &str)]) -> bool {
    #[derive(Debug, PartialEq, Clone)]
    enum Token {
        Open,
        Close,
        Not,
        Equal,
        Different,
        And,
        Or,
        Text(String),
        Name(String),
    }
    let chars: Vec<char> = expression.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let two: String = chars[at..].iter().take(2).collect();
        match chars[at] {
            ' ' => at += 1,
            '(' => {
                tokens.push(Token::Open);
                at += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                at += 1;
            }
            '\'' => {
                let end = at + 1 + chars[at + 1..].iter().position(|c| *c == '\'').unwrap();
                tokens.push(Token::Text(chars[at + 1..end].iter().collect()));
                at = end + 1;
            }
            _ if two == "==" => {
                tokens.push(Token::Equal);
                at += 2;
            }
            _ if two == "!=" => {
                tokens.push(Token::Different);
                at += 2;
            }
            _ if two == "&&" => {
                tokens.push(Token::And);
                at += 2;
            }
            _ if two == "||" => {
                tokens.push(Token::Or);
                at += 2;
            }
            '!' => {
                tokens.push(Token::Not);
                at += 1;
            }
            c if c.is_alphanumeric() || "._-".contains(c) => {
                let end = at
                    + chars[at..]
                        .iter()
                        .take_while(|c| c.is_alphanumeric() || "._-".contains(**c))
                        .count();
                let mut name: String = chars[at..end].iter().collect();
                at = end;
                if chars[at..].starts_with(&['(', ')']) {
                    name.push_str("()");
                    at += 2;
                }
                tokens.push(Token::Name(name));
            }
            c => panic!("{c:?} in {expression}"),
        }
    }

    // or := and ("||" and)*, and := not ("&&" not)*, not := "!" not | compare,
    // compare := atom (("==" | "!=") atom)?, atom := "(" or ")" | text | name.
    struct Parser<'a> {
        tokens: Vec<Token>,
        at: usize,
        context: &'a [(&'a str, &'a str)],
    }
    impl Parser<'_> {
        fn eat(&mut self, token: &Token) -> bool {
            let found = self.tokens.get(self.at) == Some(token);
            self.at += usize::from(found);
            found
        }
        fn truthy(value: &str) -> bool {
            !matches!(value, "" | "false" | "0")
        }
        fn or(&mut self) -> bool {
            let mut value = self.and();
            while self.eat(&Token::Or) {
                let next = self.and();
                value = value || next;
            }
            value
        }
        fn and(&mut self) -> bool {
            let mut value = self.not();
            while self.eat(&Token::And) {
                let next = self.not();
                value = value && next;
            }
            value
        }
        fn not(&mut self) -> bool {
            if self.eat(&Token::Not) {
                return !self.not();
            }
            let left = self.atom();
            if self.eat(&Token::Equal) {
                return left == self.atom();
            }
            if self.eat(&Token::Different) {
                return left != self.atom();
            }
            Self::truthy(&left)
        }
        fn atom(&mut self) -> String {
            if self.eat(&Token::Open) {
                let value = self.or();
                assert!(self.eat(&Token::Close), "a closing parenthesis");
                return value.to_string();
            }
            match self.tokens.get(self.at).cloned() {
                Some(Token::Text(text)) => {
                    self.at += 1;
                    text
                }
                Some(Token::Name(name)) => {
                    self.at += 1;
                    match name.as_str() {
                        "cancelled()" => "false".into(),
                        "always()" => "true".into(),
                        _ => self
                            .context
                            .iter()
                            .find(|(key, _)| *key == name)
                            .unwrap_or_else(|| panic!("the condition reads {name}"))
                            .1
                            .to_owned(),
                    }
                }
                other => panic!("{other:?}"),
            }
        }
    }
    let mut parser = Parser {
        tokens,
        at: 0,
        context,
    };
    let value = parser.or();
    assert_eq!(parser.at, parser.tokens.len(), "{expression}");
    value
}

/// The text of the step of `job` named `name`: from its `- name:` line to the next step's.
fn step_of(job: &str, name: &str) -> String {
    let lines: Vec<&str> = job.lines().collect();
    let start = lines
        .iter()
        .position(|l| *l == format!("      - name: {name}"))
        .unwrap_or_else(|| panic!("no step {name}"));
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with("      - "))
        .map_or(lines.len(), |at| start + 1 + at);
    lines[start..end].join("\n")
}

#[test]
fn android_sign_runs_on_a_push_and_on_a_release_build_of_main_only() {
    // A release build of main is `Release`'s workflow_dispatch of ci.yml on main: a commit main
    // holds is always a released commit of dev. A run started by hand on any other ref, a stable
    // line's tag included, a pull request's run, and a build that failed reach no key.
    let runs = [
        ("push", "refs/heads/main", true),
        ("push", "refs/heads/dev", true),
        ("push", "refs/heads/stable/0.4", true),
        ("workflow_dispatch", "refs/heads/main", true),
        ("workflow_dispatch", "refs/heads/dev", false),
        ("workflow_dispatch", "refs/heads/fix/1-a", false),
        ("workflow_dispatch", "refs/heads/next/0.5", false),
        ("workflow_dispatch", "refs/heads/stable/0.4", false),
        ("workflow_dispatch", "refs/tags/v0.4.1", false),
        ("pull_request", "refs/pull/7/merge", false),
    ];
    let sign = condition_of(&ci_job("android-sign"));
    let attach = condition_of(&step_of(&ci_job("publish"), "The signed APK"));
    for (event, git_ref, signs) in runs {
        let context = [
            ("github.event_name", event),
            ("github.ref", git_ref),
            ("needs.android.result", "success"),
        ];
        assert_eq!(
            holds(&sign, &context),
            signs,
            "android-sign: {event} {git_ref}"
        );
        // `publish` attaches the signed APK on the very runs that signed it, no others.
        assert_eq!(
            holds(&attach, &context),
            signs,
            "publish: {event} {git_ref}"
        );
    }
    for result in ["failure", "skipped", "cancelled"] {
        let context = [
            ("github.event_name", "push"),
            ("github.ref", "refs/heads/main"),
            ("needs.android.result", result),
        ];
        assert!(!holds(&sign, &context), "android {result}");
    }
}

#[test]
fn a_pull_request_writes_no_cache() {
    // A pull request's cache is read by that pull request alone, and the repository holds 10 GB:
    // the hub's and the build's of each pull request (1.3 GB) evicted dev's, which every run
    // reads (10.7 GB held on 23 November 2026). A pull request reads dev's and writes none.
    let workflow = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    let lines: Vec<&str> = workflow.lines().collect();
    let mut caches = 0;
    for (at, line) in lines.iter().enumerate() {
        if !line.contains("uses: Swatinem/rust-cache@v2") && !line.contains("uses: actions/cache") {
            continue;
        }
        caches += 1;
        let step: Vec<&str> = lines[at + 1..]
            .iter()
            .take_while(|l| !l.trim_start().starts_with("- "))
            .copied()
            .collect();
        let step = step.join("\n");
        if line.contains("actions/cache/restore") {
            continue;
        }
        assert!(
            !line.contains("uses: actions/cache@"),
            "a cache that restores and saves in one step saves on a pull request: {line}"
        );
        if line.contains("actions/cache/save") {
            let condition = condition_of(&format!("{line}\n{step}"));
            let context = |event: &'static str| [("github.event_name", event)];
            assert!(holds(&condition, &context("push")), "{condition}");
            assert!(!holds(&condition, &context("pull_request")), "{condition}");
            continue;
        }
        let save = step
            .lines()
            .find_map(|l| l.trim().strip_prefix("save-if:"))
            .unwrap_or_else(|| panic!("a rust-cache with no save-if, line {}: {step}", at + 1))
            .trim();
        assert!(
            save == "false" || save == "${{ github.event_name != 'pull_request' }}",
            "line {}: {save}",
            at + 1
        );
    }
    assert!(caches >= 6, "{caches} caches");
}

#[test]
fn the_android_job_keeps_gradle_s_downloads_between_runs() {
    // Maven Central answered the Robolectric jar's download with 429, Too Many Requests, three
    // times in a row: the Android job failed dev six times in two days (21-23 November 2026).
    // Gradle's downloads are kept from one run to the next: the jar is fetched once per version.
    let android = ci_job("android");
    let restore = android
        .find("uses: actions/cache/restore@v4")
        .expect("the downloads are restored");
    let run = android
        .find("apps/android/ci.sh")
        .expect("the job runs ci.sh");
    assert!(restore < run, "{android}");
    let save = android
        .find("uses: actions/cache/save@v4")
        .expect("the downloads are kept");
    assert!(run < save, "{android}");
    assert_eq!(
        android.matches("~/.gradle/caches/modules-2").count(),
        2,
        "the same folder restored and saved: {android}"
    );
}

/// Every file under `dir`, folders walked.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
fn the_android_tests_never_fetch_at_test_time() {
    let gradle = std::fs::read_to_string(tool("apps/android/app/build.gradle.kts")).unwrap();
    // The jars Robolectric loads are a Gradle dependency, resolved to a folder by a task of their
    // own, and every test task is told to read that folder and to fetch nothing.
    assert!(
        gradle.contains("org.robolectric:android-all-instrumented:"),
        "the jars are a dependency: {gradle}"
    );
    assert!(
        gradle.contains("\"robolectric.offline\", \"true\""),
        "{gradle}"
    );
    assert!(
        gradle.contains("\"robolectric.dependency.dir\""),
        "{gradle}"
    );
    assert!(
        gradle.contains("dependsOn(fetchRobolectricJars)"),
        "a test never runs before its jars are there: {gradle}"
    );

    // One jar per SDK a test can run on: the SDKs the tests name (`@Config(sdk = [..])`) and the
    // app's `targetSdk`, which Robolectric asks for too (the first run of this job found 35's
    // missing: every test failed with "Path is not a file"). The version of each is the one
    // Robolectric 4.14.1's `DefaultSdkProvider` names for it.
    let jar = |sdk: u32| match sdk {
        34 => "14-robolectric-10818077-i7",
        35 => "15-robolectric-12650502-i7",
        other => panic!("SDK {other}: add its Robolectric version to this test"),
    };
    let mut sdks = std::collections::BTreeSet::new();
    let target = gradle
        .lines()
        .find_map(|l| l.trim().strip_prefix("targetSdk = "))
        .expect("a targetSdk");
    sdks.insert(target.trim().parse::<u32>().unwrap());
    let tests = tool("apps/android/app/src/test");
    for entry in files_under(&tests) {
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        for list in text.split("sdk = [").skip(1) {
            let list = &list[..list.find(']').unwrap()];
            sdks.extend(list.split(',').map(|n| n.trim().parse::<u32>().unwrap()));
        }
    }
    for sdk in sdks {
        assert!(
            gradle.contains(&format!("\"{}\"", jar(sdk))),
            "the jar of SDK {sdk} is a dependency: {gradle}"
        );
    }
    // Each in a configuration of its own: in one, Gradle keeps the newest version of the module
    // and drops the others (the second run of this job found only SDK 35's jar in the folder).
    assert!(gradle.contains("detachedConfiguration("), "{gradle}");
    assert!(!gradle.contains("robolectricJars("), "{gradle}");

    // The job fetches them in a step of their own, before the tests, which may retry; the tests
    // are not run with the network's help.
    let script = std::fs::read_to_string(tool("apps/android/ci.sh")).unwrap();
    let fetch = script
        .find(":app:fetchRobolectricJars")
        .expect("ci.sh fetches the jars");
    let tests = script
        .find(":app:testDebugUnitTest")
        .expect("ci.sh runs the unit tests");
    assert!(fetch < tests, "the jars before the tests: {script}");
    assert!(
        script[..fetch].contains("for attempt in"),
        "a network hiccup is retried: {script}"
    );
    assert!(
        script[..fetch].contains("the Robolectric jars"),
        "the step says what it does: {script}"
    );
    assert!(
        ci_job("android").contains("apps/android/ci.sh"),
        "the job runs it"
    );
}

/// The `prune` step of `beta-cleanup.yml`, run against `FAKE_RELEASES_GH`, with no pause between
/// two deletions: the other settings of the workflow (`env`) are the ones the tests below read.
fn prune(fake: &Fake) -> Output {
    fake.run(
        &step_script(".github/workflows/beta-cleanup.yml", "prune"),
        &[
            ("GH_TOKEN", "unused"),
            ("KEEP", &prune_setting("KEEP")),
            ("MAX_DELETIONS", &prune_setting("MAX_DELETIONS")),
            ("PAUSE", "0"),
        ],
    )
}

/// The value of an `env` entry of the pruning step, as the workflow says it.
fn prune_setting(name: &str) -> String {
    let text = std::fs::read_to_string(tool(".github/workflows/beta-cleanup.yml")).unwrap();
    let prefix = format!("{name}: ");
    text.lines()
        .find_map(|l| l.trim().strip_prefix(&prefix).map(str::to_owned))
        .unwrap_or_else(|| panic!("beta-cleanup.yml sets no {name}"))
}

#[test]
fn the_betas_of_a_version_keep_the_last_ten() {
    let fake = Fake::new();
    let mut state: Vec<String> = (1..=14).map(|n| format!("v0.4.0-beta.{n}")).collect();
    // Another version: eleven, and only the oldest goes.
    state.extend((1..=11).map(|n| format!("v0.5.0-beta.{n}")));
    // Few of a version: kept. Never a stable release, a release candidate, a next build or an alpha,
    // however many.
    state.extend((1..=3).map(|n| format!("v0.3.0-beta.{n}")));
    state.extend((1..=12).map(|n| format!("v0.4.1-rc.{n}")));
    state.extend((1..=12).map(|n| format!("v0.6.0-next.{n}")));
    state.extend((1..=12).map(|n| format!("v0.4.0-alpha.feat-a.{n}")));
    state.extend(["v0.4.0".into(), "v0.3.0".into(), "v0.4.1".into()]);
    let before = state.clone();
    fake.state(&state.iter().map(String::as_str).collect::<Vec<_>>());

    let output = prune(&fake);

    assert!(output.status.success(), "{output:?}");
    let kept = fake.tags();
    // 1..=4 of 0.4.0 are gone (2 and 10 are not ordered as text), and 0.5.0's first.
    let gone: Vec<String> = before
        .iter()
        .filter(|tag| !kept.contains(tag))
        .cloned()
        .collect();
    let expected: Vec<String> = (1..=4)
        .map(|n| format!("v0.4.0-beta.{n}"))
        .chain(["v0.5.0-beta.1".to_owned()])
        .collect();
    assert_eq!(gone, expected, "kept: {kept:?}");
    // Their tags go with them.
    let log = fake.log();
    let deletes: Vec<&str> = log
        .lines()
        .filter(|l| l.starts_with("release delete"))
        .collect();
    assert_eq!(deletes.len(), expected.len(), "{log}");
    for line in deletes {
        assert!(
            line.contains("--cleanup-tag") && line.contains("--yes"),
            "{line}"
        );
    }

    // Run again, nothing more goes.
    let again = prune(&fake);
    assert!(again.status.success(), "{again:?}");
    assert_eq!(fake.tags(), kept);
}

#[test]
fn the_pruning_of_the_betas_never_touches_a_release_run() {
    let ci = std::fs::read_to_string(tool(".github/workflows/ci.yml")).unwrap();
    // In a workflow of its own, not a job of `ci`: its failure, or its cancellation by a
    // concurrency group, would fail or cancel the push run `verified-on-dev` reads, and `main`
    // would run the whole verification again.
    assert!(
        !ci.contains("prune") && !ci.contains("--cleanup-tag"),
        "ci.yml prunes nothing (it deletes its own failed draft, and only that)"
    );
    let workflow = std::fs::read_to_string(tool(".github/workflows/beta-cleanup.yml")).unwrap();
    let triggers = &workflow[..workflow.find("\npermissions:").expect("permissions")];
    // On a schedule and by hand: a release made with the workflow's token starts no `release`
    // run, and nothing here runs on a push or a pull request.
    assert!(
        triggers.contains("schedule:") && triggers.contains("workflow_dispatch:"),
        "{triggers}"
    );
    for event in ["push:", "pull_request", "workflow_run:", "release:"] {
        assert!(!triggers.contains(event), "{event} in {triggers}");
    }
    // Least privilege: it deletes releases and their tags, and nothing else.
    assert!(
        workflow.contains("permissions:\n  contents: write\n"),
        "{workflow}"
    );
    assert!(!workflow.contains("actions/"), "no action, no checkout");
    assert!(!workflow.contains("secrets."), "{workflow}");
    // Nothing of the event reaches a command.
    let script = step_script(".github/workflows/beta-cleanup.yml", "prune");
    assert!(!script.contains("${{"), "{script}");
    // One at a time, none cancelled half-way.
    assert!(workflow.contains("concurrency:"), "{workflow}");
    assert!(workflow.contains("cancel-in-progress: false"), "{workflow}");
    // The betas' tags only: a release candidate, a release or an alpha is never listed.
    assert!(script.contains("-beta\\."), "{script}");
}

#[test]
fn the_pruning_deletes_a_few_betas_per_run_with_a_pause_between_calls() {
    let fake = Fake::new();
    // The pile of today: far more than a run may delete (GitHub's secondary rate limit).
    let state: Vec<String> = (1..=80).map(|n| format!("v0.2.0-beta.{n}")).collect();
    fake.state(&state.iter().map(String::as_str).collect::<Vec<_>>());
    let per_run: usize = prune_setting("MAX_DELETIONS").parse().expect("a number");
    assert_eq!(per_run, 30, "the issue says thirty");

    // 70 are older than the last ten: thirty go, then thirty, then ten, then nothing.
    let mut gone = 0;
    for (run, expected) in [(1, 30), (2, 30), (3, 10), (4, 0)] {
        let output = prune(&fake);
        assert!(output.status.success(), "run {run}: {output:?}");
        let now = state.len() - fake.tags().len();
        assert_eq!(now - gone, expected, "run {run}");
        gone = now;
    }
    let expected: Vec<String> = (71..=80).map(|n| format!("v0.2.0-beta.{n}")).collect();
    assert_eq!(fake.tags(), expected);

    // A pause between two calls, as the workflow says it, never none.
    let script = step_script(".github/workflows/beta-cleanup.yml", "prune");
    assert!(script.contains("sleep \"$PAUSE\""), "{script}");
    let pause: u32 = prune_setting("PAUSE").parse().expect("a number");
    assert!(pause >= 1, "a pause of {pause} s");
}
