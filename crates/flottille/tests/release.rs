//! The scenarios of the packages (#46) that can run without a push: the version computed from
//! the commits, the changelog, the archive and its sum, `install.sh` and `flottille update`
//! against a local server standing for the releases, in a throwaway home. Nothing here talks to
//! GitHub; what only a real push proves is said in `docs/releases.md`.

mod common;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
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

    /// A stable release as CI makes it: dev squashed into main, `v<version>` on the squash and
    /// the mark `released/v<version>` on the head of dev it squashed. Back on dev.
    fn release(&self, version: &str) {
        let dev = self.squash_dev_into_main();
        self.tag(&format!("v{version}"));
        self.git(&["tag", &format!("released/v{version}"), &dev]);
        self.git(&["checkout", "--quiet", "dev"]);
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
fn a_feat_since_the_last_stable_tag_bumps_the_minor() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("docs(repo): say more");
    repo.commit("feat(gui): add a page (#12)");
    repo.commit("fix(cli): a typo (#13)");

    assert_eq!(repo.next(&["stable"]), "0.4.0");
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
fn before_1_0_a_breaking_change_bumps_the_minor() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.4");
    repo.commit("fix(cli)!: rename the commands");
    assert_eq!(repo.next(&["stable"]), "0.4.0");
}

#[test]
fn without_a_stable_tag_the_versions_count_from_0_0_0() {
    let repo = Repo::new();
    repo.commit("chore(repo): start");
    repo.commit("feat(core): the first thing");
    assert_eq!(repo.next(&["stable"]), "0.1.0");
    assert_eq!(repo.next(&["beta"]), "0.1.0-beta.2");
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
    assert_eq!(repo.next(&["beta"]), "0.4.0-beta.2");
    repo.commit("docs(repo): say more (#15)");
    assert_eq!(repo.next(&["beta"]), "0.4.0-beta.3");
}

#[test]
fn pre_release_tags_are_not_stable_ones() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.commit("feat(gui): add a page");
    repo.tag("v0.4.0-beta.1");
    repo.commit("fix(cli): a typo");

    assert_eq!(repo.next(&["stable"]), "0.4.0");
    assert_eq!(repo.next(&["beta"]), "0.4.0-beta.2");
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
        "0.4.0-alpha.feat-46-packages.2"
    );
    // Its name as git has it when none is given.
    assert_eq!(repo.next(&["alpha"]), "0.4.0-alpha.feat-46-packages.2");
}

#[test]
fn the_stable_version_is_the_beta_of_the_squashed_dev_head_without_its_pre_release() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#14)");
    repo.commit("fix(cli): a typo (#15)");
    assert_eq!(repo.next(&["beta"]), "0.4.0-beta.2");

    let dev = repo.squash_dev_into_main();

    assert_eq!(repo.next(&["stable", "--at", &dev]), "0.4.0");
}

#[test]
fn after_dev_is_squashed_into_main_the_next_beta_counts_from_the_released_mark() {
    let repo = Repo::new();
    repo.commit("feat(core): start");
    repo.tag("v0.3.0");
    repo.git(&["checkout", "--quiet", "-b", "dev"]);
    repo.commit("feat(gui): add a page (#14)");
    repo.commit("fix(cli): a typo (#15)");
    repo.release("0.4.0");
    repo.commit("fix(store): a lock (#16)");
    assert_eq!(repo.next(&["beta"]), "0.4.1-beta.1");

    // And again: the second squash counts from the second mark.
    repo.commit("feat(gui): another page (#17)");
    assert_eq!(repo.next(&["beta"]), "0.5.0-beta.2");
    repo.release("0.5.0");
    repo.commit("docs(repo): say more (#18)");

    assert_eq!(repo.next(&["beta"]), "0.5.1-beta.1");
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

// The archive.

/// `tools/package.sh <version> <binary> <folder>`: the archive and its sum, in `folder`.
fn package(version: &str, binary: &Path, out: &Path) -> Output {
    Command::new(tool("tools/package.sh"))
        .args([version, binary.to_str().unwrap(), out.to_str().unwrap()])
        .output()
        .unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn the_archive_holds_the_binary_and_both_licences_with_its_sum_beside_it() {
    let out = tempfile::tempdir().unwrap();
    let binary = Path::new(env!("CARGO_BIN_EXE_flottille"));

    let output = package("0.4.0-beta.2", binary, out.path());

    assert!(output.status.success(), "{output:?}");
    let archive = out
        .path()
        .join("flottille-0.4.0-beta.2-x86_64-linux.tar.gz");
    let sum = std::fs::read_to_string(archive.with_extension("gz.sha256")).unwrap();
    assert_eq!(
        sum,
        format!(
            "{}  flottille-0.4.0-beta.2-x86_64-linux.tar.gz\n",
            sha256(&std::fs::read(&archive).unwrap())
        )
    );
    let listed = Command::new("tar")
        .args(["-tzf", archive.to_str().unwrap()])
        .output()
        .unwrap();
    let mut names: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "flottille-0.4.0-beta.2/",
            "flottille-0.4.0-beta.2/LICENSE",
            "flottille-0.4.0-beta.2/LICENSE-fonts.txt",
            "flottille-0.4.0-beta.2/flottille",
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
        Releases { base, files, asked }
    }

    /// The API's base, as `FLOTTILLE_RELEASES` takes it.
    fn api(&self) -> String {
        format!("{}/repos/o/r", self.base)
    }

    fn put(&self, path: &str, body: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_owned(), body);
    }

    /// A release of that tag, holding the archive of `binary` at `version` and its sum (`sum`
    /// in place of the right one when given). Its API entry.
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
        json!({
            "tag_name": tag,
            "prerelease": tag.contains('-'),
            "assets": [
                {"name": name, "browser_download_url": url(&name)},
                {"name": format!("{name}.sha256"), "browser_download_url": url(&format!("{name}.sha256"))},
            ],
        })
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
    releases.put(
        "/repos/o/r/releases?per_page=100",
        json!([alpha, beta, stable]).to_string().into_bytes(),
    );
    releases.put(
        "/repos/o/r/releases/tags/alpha-feat-46-packages",
        alpha.to_string().into_bytes(),
    );
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

/// Two betas listed newest first by date, the older version first: beta.10 is the newest by
/// version.
fn betas_out_of_order(releases: &Releases, programs: &Path) {
    let nine = releases.release(
        "v0.4.1-beta.9",
        "0.4.1-beta.9",
        &standing_for(programs, "0.4.1-beta.9"),
        None,
    );
    let ten = releases.release(
        "v0.4.1-beta.10",
        "0.4.1-beta.10",
        &standing_for(programs, "0.4.1-beta.10"),
        None,
    );
    releases.put(
        "/repos/o/r/releases?per_page=100",
        json!([nine, ten]).to_string().into_bytes(),
    );
}

#[test]
fn install_sh_picks_the_newest_beta_by_version_not_by_date() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    betas_out_of_order(&releases, programs.path());
    let home = tempfile::tempdir().unwrap();

    let output = install(home.path(), &releases, &["--channel", "beta"]);

    assert!(output.status.success(), "{output:?}");
    let said = Command::new(home.path().join(".local/bin/flottille"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&said.stdout).trim(),
        "flottille 0.4.1-beta.10 (test, 000000000000)"
    );
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

#[test]
fn the_branch_name_is_sanitised_the_same_way_everywhere() {
    // Each byte but ASCII letters, digits and dashes is a dash: `é` is two.
    let branch = "feat/é_x";
    let tag = "alpha-feat----x";

    let repo = Repo::new();
    repo.commit("feat(core): start");
    assert_eq!(
        repo.next(&["alpha", "--branch", branch]),
        "0.1.0-alpha.feat----x.1"
    );

    let releases = Releases::serve();
    let home = tempfile::tempdir().unwrap();
    install(
        home.path(),
        &releases,
        &["--channel", "alpha", "--branch", branch],
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed(&user);
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

/// A copy of Flottille's binary in the user's folder, the one `update` replaces.
fn installed(user: &User) -> PathBuf {
    let bin = user.path("home/.local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join("flottille");
    // Copied by `cp`, not by this process: a file it held open for writing would be inherited by
    // what another test spawns at that moment, and running it would fail with "Text file busy".
    let copied = Command::new("cp")
        .arg(env!("CARGO_BIN_EXE_flottille"))
        .arg(&path)
        .status()
        .unwrap();
    assert!(copied.success());
    path
}

#[test]
fn flottille_update_replaces_the_binary_from_a_local_fake_release() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    three_channels(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed(&user);

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
    let binary = installed(&user);
    let before = std::fs::read(&binary).unwrap();

    let output = user
        .command_through(&binary, &["update", "--channel", "stable"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("SHA-256"),
        "{output:?}"
    );
    assert_eq!(std::fs::read(&binary).unwrap(), before);
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
    let binary = installed(&user);
    let before = std::fs::read(&binary).unwrap();

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
    assert_eq!(std::fs::read(&binary).unwrap(), before);
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
    let binary = installed(&user);
    let before = std::fs::read(&binary).unwrap();

    let output = user
        .command_through(&binary, &["update", "--channel", "beta"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("`flottille daemon stop`"),
        "{output:?}"
    );
    assert_eq!(std::fs::read(&binary).unwrap(), before);
    assert!(
        releases.asked.lock().unwrap().is_empty(),
        "nothing was read"
    );
}

#[test]
fn flottille_update_picks_the_newest_beta_by_version_not_by_date() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    betas_out_of_order(&releases, programs.path());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed(&user);

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
    let binary = installed(&user);
    let before = std::fs::read(&binary).unwrap();

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
    assert_eq!(std::fs::read(&binary).unwrap(), before);

    let output = user
        .command_through(&binary, &["update", "--channel", "stable", "--force"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_ne!(std::fs::read(&binary).unwrap(), before);
}

#[test]
fn flottille_update_refuses_an_older_version_without_force() {
    let releases = Releases::serve();
    let programs = tempfile::tempdir().unwrap();
    stable_at(&releases, programs.path(), "0.0.1");
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let binary = installed(&user);
    let before = std::fs::read(&binary).unwrap();

    let output = user
        .command_through(&binary, &["update", "--channel", "stable"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("0.0.1 is older"), "{said}");
    assert!(said.contains("--force"), "{said}");
    assert_eq!(std::fs::read(&binary).unwrap(), before);

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
    let binary = installed(&user);

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
    let binary = installed(&user);

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
    let binary = installed(&user);

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
