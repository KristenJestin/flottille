//! The scenarios of the packages (#46) that can run without a push: the version computed from
//! the commits, the changelog, the archive and its sum, `install.sh` and `flottille update`
//! against a local server standing for the releases, in a throwaway home. Nothing here talks to
//! GitHub; what only a real push proves is said in `docs/releases.md`.

mod common;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
}

impl Repo {
    fn new() -> Repo {
        let repo = Repo {
            dir: tempfile::tempdir().unwrap(),
        };
        repo.git(&["init", "--quiet", "--initial-branch=main"]);
        repo
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

    /// `tools/changelog.sh` in this repository.
    fn changelog(&self, args: &[&str]) -> String {
        let output = Command::new(tool("tools/changelog.sh"))
            .args(args)
            .current_dir(self.dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
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
        "### Features\n\n- **tools**: cut long results (#350)\n\n\
         ### Fixes\n\n- **store**: a lock (#351)\n"
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
        "### Other changes\n\n- **repo**: say more (#352)\n"
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
        "### Fixes\n\n- **cli**: a flag (#360)\n\n\
         ### Performance\n\n- **store**: fewer reads (#361)\n"
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

    let output = Command::new(tool("tools/changelog.sh"))
        .current_dir(repo.dir.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "### Breaking changes\n\n- **store**: a new schema (#16)\n\n\
         ### Features\n\n- **gui**: add a page (#14)\n- **store**: a new schema (#16)\n\n\
         ### Fixes\n\n- **cli**: a typo (#13)\n\n\
         ### Performance\n\n- **store**: fewer reads (#17)\n\n\
         ### Other changes\n\n- **repo**: say more (#15)\n"
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
        "### Features\n\n- **gui**: add a page (#14)\n\n\
         ### Other changes\n\n- **repo**: say more (#15)\n"
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
        "### Fixes\n\n- **cli**: a typo (#21)\n\n\
         ### Other changes\n\n- **repo**: say more (#22)\n"
    );
}

// The archive.

/// `tools/package.sh <version> <binary> <folder>`: the archive and its sum, in `folder`.
fn package(version: &str, binary: &Path, out: &Path) -> Output {
    Command::new(tool("tools/package.sh"))
        .args([version, binary.to_str().unwrap(), out.to_str().unwrap()])
        .output()
        .unwrap()
}

/// The SHA-256 of a file. It is read in chunks: the `flottille` binary weighs over a gigabyte in
/// a debug build and the tests run side by side, so none of them may hold it whole.
fn sha256(path: impl AsRef<Path>) -> String {
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
struct Releases {
    base: String,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    asked: Arc<Mutex<Vec<String>>>,
    /// Each release made is published a second after the previous one.
    published: AtomicUsize,
}

impl Releases {
    fn serve() -> Releases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let (served, noted) = (files.clone(), asked.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                noted.lock().unwrap().push(path.clone());
                let body = served.lock().unwrap().get(&path).cloned();
                let _ = match body {
                    Some(body) => write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .and_then(|()| stream.write_all(&body)),
                    None => write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    ),
                };
            }
        });
        Releases {
            base,
            files,
            asked,
            published: AtomicUsize::new(0),
        }
    }

    /// The API's base, as `FLOTTILLE_RELEASES` takes it.
    fn api(&self) -> String {
        format!("{}/repos/o/r", self.base)
    }

    fn put(&self, path: &str, body: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_owned(), body);
    }

    /// Takes the address away: the API answers there not found, as it does for a deleted release.
    fn delete(&self, path: &str) {
        self.files.lock().unwrap().remove(path);
    }

    /// The list the API answers in GitHub's own layout, not `list`'s short one: `author` before
    /// the tag, `published_at` (`null` on a draft) before the assets, an `uploader` object in each
    /// asset, and a body of commas and escaped quotes at the end.
    fn list_as_github(&self, releases: &[&serde_json::Value]) {
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
    fn list(&self, releases: &[&serde_json::Value]) {
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
    fn release(
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
fn standing_for(dir: &Path, version: &str) -> PathBuf {
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
        "alpha-feat-46-packages",
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

/// An alpha holding two archives while it is replaced, the older one listed first.
fn alpha_with_two_archives(releases: &Releases, programs: &Path) {
    let old = releases.release(
        "alpha-feat-x",
        "0.5.0-alpha.feat-x.7",
        &standing_for(programs, "0.5.0-alpha.feat-x.7"),
        None,
    );
    let new = releases.release(
        "alpha-feat-x",
        "0.5.0-alpha.feat-x.8",
        &standing_for(programs, "0.5.0-alpha.feat-x.8"),
        None,
    );
    let mut both = old.clone();
    both["assets"]
        .as_array_mut()
        .unwrap()
        .extend(new["assets"].as_array().unwrap().iter().cloned());
    releases.put(
        "/repos/o/r/releases/tags/alpha-feat-x",
        both.to_string().into_bytes(),
    );
}

#[test]
fn install_sh_picks_the_highest_archive_of_an_alpha_being_replaced() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    alpha_with_two_archives(&releases, programs.path());
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
        "flottille 0.5.0-alpha.feat-x.8 (test, 000000000000)"
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
cd "$FAKE_GH_REPO"
exists() { git cat-file -e "$1^{commit}" 2> /dev/null; }
case "$1 ${2:-}" in
  "api repos/o/r/git/ref/heads/"*)
    git rev-parse --verify -q "refs/heads/${2#repos/o/r/git/ref/heads/}" || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; } ;;
  "api repos/o/r/compare/"*)
    range=${2#repos/o/r/compare/} base=${range%%...*} head=${range#*...}
    exists "$base" && exists "$head" || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
    if [ "$(git rev-parse "$base")" = "$(git rev-parse "$head")" ]; then echo identical
    elif git merge-base --is-ancestor "$base" "$head"; then echo ahead
    elif git merge-base --is-ancestor "$head" "$base"; then echo behind
    else echo diverged; fi ;;
  "api -X")
    [ "$3 $4" = "PATCH repos/o/r/git/refs/heads/main" ] && [ "$7 $8" = "-F force=false" ] || { echo "gh: unexpected $*" >&2; exit 2; }
    sha=${6#sha=}
    exists "$sha" && git merge-base --is-ancestor main "$sha" || { echo "gh: Update is not a fast forward (HTTP 422)" >&2; exit 1; }
    git update-ref refs/heads/main "$sha"
    echo '{}' ;;
  "workflow run") ;;
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
        .env("FAKE_GH_REPO", repo.dir.path())
        .env("FAKE_GH_LOG", &log)
        .env("GH_REPO", "o/r")
        .env("GITHUB_REF", git_ref)
        .env("COMMIT", commit)
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

/// `ci.yml`'s step that computes the version, run as on a run of main at `sha`, in a clone of
/// the repository with its `origin`. Its outputs, or its failure.
fn version_step_on_main(repo: &Repo, sha: &str) -> (Output, String) {
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
    let output = Command::new("bash")
        .args(["--noprofile", "--norc", "-eo", "pipefail"])
        .arg(&script)
        .current_dir(clone.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GITHUB_REF_NAME", "main")
        .env("GITHUB_SHA", sha)
        .env("GITHUB_OUTPUT", &outputs)
        .env("RUNNER_TEMP", temp.path())
        .output()
        .unwrap();
    (output, std::fs::read_to_string(&outputs).unwrap())
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
fn the_branch_name_is_sanitised_the_same_way_everywhere() {
    // Each byte but ASCII letters, digits and dashes is a dash: `é` is two.
    let branch = "feat/é_x";
    let tag = "alpha-feat----x";

    let repo = Repo::new();
    repo.commit("feat(core): start");
    assert_eq!(
        repo.next(&["alpha", "--branch", branch]),
        "0.0.1-alpha.feat----x.1"
    );

    let releases = Releases::serve();
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        &releases,
        &["--channel", "alpha", "--branch", branch],
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed();
    user.command_through(
        &binary,
        &["update", "--channel", "alpha", "--branch", branch],
    )
    .output()
    .unwrap();
    let asked: Vec<String> = releases
        .asked
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.contains("/releases/tags/"))
        .cloned()
        .collect();
    assert_eq!(
        asked,
        [
            format!("/repos/o/r/releases/tags/{tag}"),
            format!("/repos/o/r/releases/tags/{tag}")
        ]
    );

    // The workflows have no test of their own: they say the same rule, word for word.
    for workflow in [
        ".github/workflows/ci.yml",
        ".github/workflows/alpha-cleanup.yml",
    ] {
        let text = std::fs::read_to_string(tool(workflow)).unwrap();
        assert!(
            text.contains("LC_ALL=C tr -c 'A-Za-z0-9-' '-'"),
            "{workflow} sanitises the branch another way"
        );
    }
}

// `flottille update`.

/// Flottille's binary in a folder of its own, the one `update` replaces.
struct Installed {
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
fn installed() -> Installed {
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
    assert_eq!(
        plain_update_reads(),
        ["/repos/o/r/releases/tags/alpha-feat-46-packages"]
    );

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
    releases.delete("/repos/o/r/releases/tags/alpha-feat-46-packages");

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
fn this_version() -> String {
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
fn stable_at(releases: &Releases, programs: &Path, version: &str) {
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
fn flottille_update_picks_the_highest_archive_of_an_alpha_being_replaced() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    alpha_with_two_archives(&releases, programs.path());
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
        "flottille 0.5.0-alpha.feat-x.8 (test, 000000000000)"
    );
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
