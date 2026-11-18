//! A place's code on its way to another machine and its new commits on their way back
//! (`docs/remote.md`, "What travels"): git bundles written and read by the user's own `git`, from
//! files only. Nothing here pushes, fetches from a remote or asks anything; what the other side
//! has is a list of object ids its caller carries.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use super::{Class, Output};

/// The uncommitted work of one repository, made a commit on top of `HEAD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The branch `HEAD` is on: the name the other side's workspace takes.
    pub branch: String,
    /// The commit `HEAD` was on, the snapshot's parent.
    pub head: String,
    /// The snapshot, held by [`outgoing_ref`].
    pub commit: String,
}

/// What a bundle that was asked for gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bundled {
    /// The bundle file is written.
    Written,
    /// The other side has every commit already: git makes no empty bundle, and no file is written.
    NothingNew,
}

/// What a received bundle laid in the repository, under `refs/flottille/incoming/<launch>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// The sender's snapshot.
    pub snapshot: String,
    /// The commit the sender's branch was on: the base of the workspace's branch.
    pub tip: String,
}

/// The ref that holds a launch's snapshot on the sending side.
pub fn outgoing_ref(launch: &str) -> String {
    format!("refs/flottille/outgoing/{launch}")
}

/// The ref that holds a launch's snapshot on the receiving side, until its workspace is made.
pub fn incoming_ref(launch: &str) -> String {
    format!("refs/flottille/incoming/{launch}")
}

/// The ref a machine's new commits on `branch` come back to.
pub fn machines_ref(machine: &str, branch: &str) -> String {
    format!("refs/flottille/machines/{machine}/{branch}")
}

/// Makes the changed and untracked files of `repository` that are not ignored one commit on top
/// of `HEAD`, held by [`outgoing_ref`]. It is made from a copy of the index: the user's branch,
/// index, files, stash and `HEAD`'s reflog are not touched.
pub async fn snapshot(repository: &Path, launch: &str) -> Result<Snapshot, String> {
    let reference = outgoing_ref(launch);
    one_component(repository, "launch", launch, &reference).await?;
    whole(repository).await?;
    let on = run_git(
        repository,
        &["symbolic-ref", "--quiet", "HEAD"],
        Class::Read,
    )
    .await?;
    if !on.success {
        return Err(format!("{} is not on a branch", repository.display()));
    }
    // `--short` would say `heads/<branch>` when a tag has the branch's name.
    let Some(branch) = on.stdout.trim().strip_prefix("refs/heads/") else {
        return Err(format!("{} is not on a branch", repository.display()));
    };
    let branch = branch.to_owned();
    let head = commit_of(repository, "HEAD")
        .await?
        .ok_or_else(|| format!("{} has no commit yet", repository.display()))?;
    // A linked worktree's index is in its own git folder.
    let index = output_git(
        repository,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        Class::Read,
    )
    .await?;
    let index = Path::new(index.trim_end_matches('\n'));
    let scratch = tempfile::tempdir().map_err(|error| format!("cannot make a folder: {error}"))?;
    let copy = scratch.path().join("index");
    // The copy keeps what the index knows that a tree does not (sparse entries, stat data); a
    // repository with no index yet starts from none.
    if index.exists() {
        std::fs::copy(index, &copy)
            .map_err(|error| format!("cannot copy {}: {error}", index.display()))?;
    }
    let env = [("GIT_INDEX_FILE", copy.as_os_str())];
    output_git_with(repository, &["add", "--all"], Class::Work, &env).await?;
    let tree = output_git_with(repository, &["write-tree"], Class::Work, &env).await?;
    // The snapshot is laid as uncommitted changes, never kept as a commit: it carries Flottille's
    // name rather than needing the user's, and is never signed.
    let name = OsStr::new("Flottille");
    let email = OsStr::new("flottille@localhost");
    let identity = [
        ("GIT_AUTHOR_NAME", name),
        ("GIT_AUTHOR_EMAIL", email),
        ("GIT_COMMITTER_NAME", name),
        ("GIT_COMMITTER_EMAIL", email),
    ];
    let message = format!("Uncommitted work of {branch}");
    let commit = output_git_with(
        repository,
        &[
            "commit-tree",
            "--no-gpg-sign",
            "-p",
            &head,
            "-m",
            &message,
            tree.trim(),
        ],
        Class::Read,
        &identity,
    )
    .await?;
    let commit = commit.trim().to_owned();
    output_git(
        repository,
        &["update-ref", "--end-of-options", &reference, &commit],
        Class::Read,
    )
    .await?;
    Ok(Snapshot {
        branch,
        head,
        commit,
    })
}

/// Writes to `file` a bundle of the launch's snapshot and the history of its branch below it,
/// past the commits of `has` that this repository knows (the bundle's prerequisites; an id it
/// does not know is left out). The snapshot's parent is the branch as it was when it was made.
pub async fn outgoing(
    repository: &Path,
    launch: &str,
    has: &[String],
    file: &Path,
) -> Result<Bundled, String> {
    let reference = outgoing_ref(launch);
    one_component(repository, "launch", launch, &reference).await?;
    whole(repository).await?;
    if commit_of(repository, &reference).await?.is_none() {
        return Err(format!("launch {launch} has no snapshot"));
    }
    bundle(repository, &reference, has, file).await
}

/// Writes to `file` a bundle of the commits of `branch` past those of `has` (what this side was
/// sent), for their way back.
pub async fn returning(
    repository: &Path,
    branch: &str,
    has: &[String],
    file: &Path,
) -> Result<Bundled, String> {
    check_branch(repository, branch).await?;
    whole(repository).await?;
    let reference = format!("refs/heads/{branch}");
    if commit_of(repository, &reference).await?.is_none() {
        return Err(format!("{} has no branch {branch}", repository.display()));
    }
    bundle(repository, &reference, has, file).await
}

async fn bundle(
    repository: &Path,
    reference: &str,
    has: &[String],
    file: &Path,
) -> Result<Bundled, String> {
    let mut known = Vec::new();
    for id in has.iter().filter(|id| object_id(id)) {
        if commit_of(repository, id).await?.is_some() {
            known.push(id.as_str());
        }
    }
    let mut revisions = vec![reference];
    if !known.is_empty() {
        revisions.push("--not");
        revisions.extend(&known);
    }
    let mut count = vec!["rev-list", "--count"];
    count.extend(&revisions);
    if output_git(repository, &count, Class::Work).await?.trim() == "0" {
        return Ok(Bundled::NothingNew);
    }
    let mut create: Vec<OsString> = ["bundle", "create", "--quiet", "--end-of-options"]
        .map(OsString::from)
        .into();
    create.push(file.as_os_str().to_owned());
    create.extend(revisions.iter().map(OsString::from));
    output_git(repository, &create, Class::Work).await?;
    Ok(Bundled::Written)
}

/// Whether `bundle` can be fetched in `repository`: a bundle git can read, whose prerequisites
/// the repository has.
pub async fn verify(repository: &Path, bundle: &Path) -> Result<(), String> {
    whole(repository).await?;
    let args: [&OsStr; 5] = [
        "bundle".as_ref(),
        "verify".as_ref(),
        "--quiet".as_ref(),
        "--end-of-options".as_ref(),
        bundle.as_os_str(),
    ];
    output_git(repository, &args, Class::Work)
        .await
        .map(drop)
        .map_err(|reason| format!("the bundle is refused: {reason}"))
}

/// Fetches a launch's bundle into `repository`, under [`incoming_ref`]: no branch of the
/// repository moves. A bundle refused by [`verify`], or whose pack git cannot read, fetches
/// nothing.
pub async fn receive(repository: &Path, bundle: &Path, launch: &str) -> Result<Received, String> {
    let into = incoming_ref(launch);
    one_component(repository, "launch", launch, &into).await?;
    let refspec = format!("+{}:{into}", outgoing_ref(launch));
    fetch(repository, bundle, &refspec).await?;
    let snapshot = commit_of(repository, &into)
        .await?
        .ok_or_else(|| format!("the bundle holds no snapshot of launch {launch}"))?;
    let tip = commit_of(repository, &format!("{snapshot}^"))
        .await?
        .ok_or_else(|| format!("the snapshot of launch {launch} has no parent"))?;
    Ok(Received { snapshot, tip })
}

/// A repository never received: made at `folder` from the bundle alone, never from a remote, and
/// given none. A refused bundle leaves no folder behind.
pub async fn clone(folder: &Path, bundle: &Path, launch: &str) -> Result<Received, String> {
    std::fs::create_dir(folder)
        .map_err(|error| format!("cannot make {}: {error}", folder.display()))?;
    let made = async {
        output_git(folder, &["init", "--quiet"], Class::Read).await?;
        receive(folder, bundle, launch).await
    }
    .await;
    if made.is_err() {
        let _ = std::fs::remove_dir_all(folder);
    }
    made
}

/// The first of `wanted`, `wanted-2`, `wanted-3`… that is no branch of `repository`.
pub async fn free_branch(repository: &Path, wanted: &str) -> Result<String, String> {
    check_branch(repository, wanted).await?;
    let mut name = wanted.to_owned();
    for n in 2.. {
        if commit_of(repository, &format!("refs/heads/{name}"))
            .await?
            .is_none()
        {
            break;
        }
        name = format!("{wanted}-{n}");
    }
    Ok(name)
}

/// Lays `snapshot` on the clean worktree at `worktree`, whose `HEAD` is its parent, as
/// uncommitted changes: the files as the sender's were, the index left at `HEAD`, so that a file
/// the sender changed, removed or made executable is changed, removed or executable and unstaged
/// here, and a file `HEAD` does not have is untracked.
pub async fn lay_snapshot(worktree: &Path, snapshot: &str) -> Result<(), String> {
    let head = commit_of(worktree, "HEAD").await?;
    let parent = commit_of(worktree, &format!("{snapshot}^")).await?;
    if head.is_none() || head != parent {
        return Err(format!(
            "the snapshot {snapshot} is not on top of {}",
            worktree.display()
        ));
    }
    // A change already there would be mixed with the sender's, or half laid over.
    let changed = output_git(worktree, &["status", "--porcelain"], Class::Work).await?;
    if !changed.trim().is_empty() {
        return Err(format!(
            "{} has changes of its own: the snapshot is not laid on them",
            worktree.display()
        ));
    }
    output_git(
        worktree,
        &["read-tree", "-m", "-u", "HEAD", snapshot],
        Class::Work,
    )
    .await?;
    output_git(worktree, &["reset", "--quiet"], Class::Work).await?;
    Ok(())
}

/// Fetches the commits a machine sent back into [`machines_ref`] for `branch`, never onto a
/// branch of the user's: the ref the bundle holds, whatever its name there. A name that would
/// write anywhere else is refused before anything is read.
pub async fn fetch_returned(
    repository: &Path,
    bundle: &Path,
    machine: &str,
    branch: &str,
) -> Result<String, String> {
    let into = machines_ref(machine, branch);
    one_component(repository, "machine", machine, &into).await?;
    check_branch(repository, branch).await?;
    let list: [&OsStr; 4] = [
        "bundle".as_ref(),
        "list-heads".as_ref(),
        "--end-of-options".as_ref(),
        bundle.as_os_str(),
    ];
    let heads = output_git(repository, &list, Class::Read)
        .await
        .map_err(|reason| format!("the bundle is refused: {reason}"))?;
    let sent: Vec<&str> = heads
        .lines()
        .filter_map(|line| line.split_once(' ').map(|(_, name)| name))
        .collect();
    let [theirs] = sent[..] else {
        return Err(format!(
            "the bundle holds {} refs, not one branch",
            sent.len()
        ));
    };
    if !theirs.starts_with("refs/heads/") || !valid_ref(repository, theirs).await? {
        return Err(format!("the bundle holds {theirs}, not a branch"));
    }
    fetch(repository, bundle, &format!("+{theirs}:{into}")).await?;
    Ok(into)
}

/// Verifies `bundle`, then fetches `refspec` from it, from the file alone.
async fn fetch(repository: &Path, bundle: &Path, refspec: &str) -> Result<(), String> {
    verify(repository, bundle).await?;
    let mut args: Vec<OsString> = [
        // A bundle is read through git's "file" transport: a configuration that refuses it
        // (meant for submodules and remotes) is not about a file of Flottille's own.
        "-c",
        "protocol.file.allow=always",
        "fetch",
        "--quiet",
        "--no-tags",
        "--no-write-fetch-head",
        "--no-recurse-submodules",
        // The user's repository is maintained when they say so, not by a launch's fetch.
        "--no-auto-maintenance",
        "--end-of-options",
    ]
    .map(OsString::from)
    .into();
    args.push(bundle.as_os_str().to_owned());
    args.push(refspec.into());
    // The daemon's git reaches no remote by default (`super::command`): the file alone here.
    let file = [("GIT_ALLOW_PROTOCOL", OsStr::new("file"))];
    output_git_with(repository, &args, Class::Work, &file)
        .await
        .map(drop)
        .map_err(|reason| format!("the bundle could not be fetched: {reason}"))
}

/// Set on every git this module runs: none of it reaches the network. A partial clone fetches
/// a missing object from its remote when asked about it, and the ids asked about come from the
/// other machine. Git LFS objects do not travel ("What this does not cover"), their pointers do:
/// the receiver's smudge filter downloads nothing and leaves the pointer. Clean filters still run
/// on the sending side, locally, as the user's own `git add` would run them.
fn offline() -> [(&'static str, &'static OsStr); 2] {
    [
        ("GIT_NO_LAZY_FETCH", OsStr::new("1")),
        ("GIT_LFS_SKIP_SMUDGE", OsStr::new("1")),
    ]
}

async fn run_git<S: AsRef<OsStr>>(dir: &Path, args: &[S], class: Class) -> Result<Output, String> {
    super::run_with(dir, args, class, &offline()).await
}

async fn output_git<S: AsRef<OsStr>>(
    dir: &Path,
    args: &[S],
    class: Class,
) -> Result<String, String> {
    output_git_with(dir, args, class, &[]).await
}

async fn output_git_with<S: AsRef<OsStr>>(
    dir: &Path,
    args: &[S],
    class: Class,
    env: &[(&str, &OsStr)],
) -> Result<String, String> {
    let mut all = offline().to_vec();
    all.extend_from_slice(env);
    super::output_with(dir, args, class, &all).await
}

/// Refuses a partial clone: its missing objects are on its remote, which nothing here contacts,
/// and a bundle of it would lack them.
async fn whole(repository: &Path) -> Result<(), String> {
    let partial = run_git(
        repository,
        &["config", "--get", "extensions.partialClone"],
        Class::Read,
    )
    .await?
    .success;
    let promisors = run_git(
        repository,
        &[
            "config",
            "--type=bool",
            "--get-regexp",
            r"^remote\..*\.promisor$",
        ],
        Class::Read,
    )
    .await?;
    if partial || promisors.stdout.lines().any(|line| line.ends_with(" true")) {
        return Err(format!(
            "{} is a partial clone: what it lacks is on its remote, and nothing is fetched from it",
            repository.display()
        ));
    }
    Ok(())
}

/// Refuses `name` unless it is one component of a ref name, `reference` being the ref it makes.
async fn one_component(dir: &Path, what: &str, name: &str, reference: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || !valid_ref(dir, reference).await? {
        return Err(format!("{name:?} is not a {what} name git accepts"));
    }
    Ok(())
}

/// Refuses `branch` unless git would take it for a branch name.
async fn check_branch(dir: &Path, branch: &str) -> Result<(), String> {
    if branch.starts_with('-')
        || branch == "HEAD"
        || !valid_ref(dir, &format!("refs/heads/{branch}")).await?
    {
        return Err(format!("{branch:?} is not a branch name git accepts"));
    }
    Ok(())
}

async fn valid_ref(dir: &Path, reference: &str) -> Result<bool, String> {
    Ok(run_git(dir, &["check-ref-format", reference], Class::Read)
        .await?
        .success)
}

/// The commit `revision` leads to; none when it leads to none.
async fn commit_of(dir: &Path, revision: &str) -> Result<Option<String>, String> {
    let shown = run_git(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ],
        Class::Read,
    )
    .await?;
    Ok(shown.success.then(|| shown.stdout.trim().to_owned()))
}

/// A full object id, SHA-1 or SHA-256, as git writes it.
fn object_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// `git` as the tests' own user: no configuration of the machine's, a name for commits.
    fn git_command(dir: &Path, args: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("HOME", crate::git::tests::home())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            // Reading must not refresh the index whose bytes are compared.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_AUTHOR_NAME", "Ada")
            .env("GIT_AUTHOR_EMAIL", "ada@example.invalid")
            .env("GIT_COMMITTER_NAME", "Ada")
            .env("GIT_COMMITTER_EMAIL", "ada@example.invalid");
        command
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = git_command(dir, args).output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn executable(path: &Path, yes: bool) {
        let mode = if yes { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn is_executable(path: &Path) -> bool {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o111 != 0
    }

    /// A repository on `feature` with a first commit: files, a script, an ignore rule.
    fn repository(dir: &Path) -> PathBuf {
        let repo = dir.join("sender");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "--initial-branch=feature"]);
        write(&repo.join("a.txt"), "a\n");
        write(&repo.join("b.txt"), "b\n");
        write(&repo.join("gone.txt"), "gone\n");
        write(&repo.join("src/run.sh"), "#!/bin/sh\n");
        executable(&repo.join("src/run.sh"), true);
        write(&repo.join(".gitignore"), ".env\ntarget/\n");
        git(&repo, &["add", "--all"]);
        git(&repo, &["commit", "--quiet", "-m", "first"]);
        repo
    }

    fn commit(repo: &Path, file: &str, text: &str) -> String {
        write(&repo.join(file), text);
        git(repo, &["add", "--", file]);
        git(repo, &["commit", "--quiet", "-m", file]);
        git(repo, &["rev-parse", "HEAD"])
    }

    /// Every ref and the commit it is on.
    fn refs(repo: &Path) -> String {
        git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"])
    }

    /// The objects the repository holds, loose and packed.
    fn objects(repo: &Path) -> Vec<String> {
        git(repo, &["count-objects", "-v"])
            .lines()
            .filter(|line| line.starts_with("count:") || line.starts_with("in-pack:"))
            .map(str::to_owned)
            .collect()
    }

    /// The files of a checkout, but its git folder and what it ignores: their bytes and whether
    /// they are executable.
    fn files(root: &Path) -> BTreeMap<String, (Vec<u8>, bool)> {
        let listed = git(
            root,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        );
        listed
            .split('\0')
            .filter(|name| !name.is_empty() && root.join(name).exists())
            .map(|name| {
                let path = root.join(name);
                (
                    name.to_owned(),
                    (std::fs::read(&path).unwrap(), is_executable(&path)),
                )
            })
            .collect()
    }

    fn status(repo: &Path) -> String {
        git(repo, &["status", "--porcelain=v2", "--untracked-files=all"])
    }

    /// Everything of the user's a snapshot must leave as it was.
    fn user_state(repo: &Path) -> Vec<String> {
        let index = git(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        );
        let bytes = std::fs::read(index).unwrap();
        vec![
            format!("{bytes:?}"),
            status(repo),
            git(repo, &["stash", "list"]),
            git(repo, &["rev-parse", "HEAD"]),
            git(repo, &["symbolic-ref", "HEAD"]),
            git(repo, &["reflog", "show", "HEAD"]),
            git(repo, &["for-each-ref", "refs/heads", "refs/stash"]),
            format!("{:?}", files(repo)),
        ]
    }

    /// The prerequisites written in a bundle's header.
    fn prerequisites(bundle: &Path) -> Vec<String> {
        let bytes = std::fs::read(bundle).unwrap();
        let end = bytes.windows(2).position(|w| w == b"\n\n").unwrap();
        String::from_utf8_lossy(&bytes[..end])
            .lines()
            .filter_map(|line| line.strip_prefix('-'))
            .map(|line| line.split(' ').next().unwrap().to_owned())
            .collect()
    }

    /// Sends the work of `sender` as launch `launch` to a new repository at `receiver`, and makes
    /// its workspace at `worktree` with its snapshot laid.
    async fn travel(sender: &Path, launch: &str, receiver: &Path, worktree: &Path) -> Snapshot {
        let snapshot = snapshot(sender, launch).await.unwrap();
        let file = receiver.with_extension("bundle");
        let bundled = outgoing(sender, launch, &[], &file).await.unwrap();
        assert_eq!(bundled, Bundled::Written);
        let received = clone(receiver, &file, launch).await.unwrap();
        assert_eq!(received.snapshot, snapshot.commit);
        assert_eq!(received.tip, snapshot.head);
        make_workspace(receiver, &snapshot.branch, &received, worktree).await;
        snapshot
    }

    /// What the workspace creation does with what was received: a worktree on a free branch at
    /// the sender's tip, the snapshot laid on it.
    async fn make_workspace(
        receiver: &Path,
        wanted: &str,
        received: &Received,
        worktree: &Path,
    ) -> String {
        let branch = free_branch(receiver, wanted).await.unwrap();
        let target = worktree.to_str().unwrap();
        git(
            receiver,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                &branch,
                target,
                &received.tip,
            ],
        );
        lay_snapshot(worktree, &received.snapshot).await.unwrap();
        branch
    }

    #[tokio::test]
    async fn uncommitted_and_untracked_files_travel_and_ignored_files_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        write(&sender.join("a.txt"), "a, changed\n");
        write(&sender.join("notes/new.txt"), "new\n");
        write(&sender.join(".env"), "TOKEN=invented\n");
        write(&sender.join("target/build.o"), "binary\n");
        let worktree = dir.path().join("workspace");

        let snapshot = travel(&sender, "l1", &dir.path().join("receiver"), &worktree).await;

        let held = git(&sender, &["ls-tree", "-r", "--name-only", &snapshot.commit]);
        assert!(held.lines().any(|name| name == "notes/new.txt"), "{held}");
        assert!(
            !held.contains(".env") && !held.contains("target/"),
            "{held}"
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join("a.txt")).unwrap(),
            "a, changed\n"
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join("notes/new.txt")).unwrap(),
            "new\n"
        );
        assert!(!worktree.join(".env").exists());
        assert!(!worktree.join("target").exists());
    }

    #[tokio::test]
    async fn the_user_s_branch_index_and_files_are_untouched_by_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let main = repository(dir.path());
        // A place is a linked worktree: its index is in its own git folder.
        let linked = dir.path().join("linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "place",
                linked.to_str().unwrap(),
            ],
        );
        for (n, repo) in [&main, &linked].into_iter().enumerate() {
            write(&repo.join("stashed.txt"), "stashed\n");
            git(repo, &["stash", "push", "--quiet", "--include-untracked"]);
            // Staged, then changed again: the index differs from both HEAD and the files.
            write(&repo.join("a.txt"), "staged\n");
            git(repo, &["add", "a.txt"]);
            write(&repo.join("a.txt"), "staged, then changed\n");
            std::fs::remove_file(repo.join("gone.txt")).unwrap();
            write(&repo.join("untracked.txt"), "untracked\n");
            write(&repo.join(".env"), "TOKEN=invented\n");
            let before = user_state(repo);

            let launch = format!("l{n}");
            let made = snapshot(repo, &launch).await.unwrap();

            assert_eq!(user_state(repo), before, "{}", repo.display());
            assert_eq!(made.head, git(repo, &["rev-parse", "HEAD"]));
            assert_eq!(
                git(repo, &["rev-parse", &outgoing_ref(&launch)]),
                made.commit
            );
            assert_eq!(
                git(repo, &["show", &format!("{}:a.txt", made.commit)]),
                "staged, then changed"
            );
            let held = git(repo, &["ls-tree", "-r", "--name-only", &made.commit]);
            assert!(!held.lines().any(|name| name == "gone.txt"), "{held}");
            assert!(held.lines().any(|name| name == "untracked.txt"), "{held}");
        }
    }

    #[tokio::test]
    async fn only_the_commits_the_other_side_lacks_are_bundled() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        let receiver = dir.path().join("receiver");
        travel(&sender, "l1", &receiver, &dir.path().join("w1")).await;
        commit(&sender, "c2.txt", "2\n");
        let has: Vec<String> = git(&receiver, &["for-each-ref", "--format=%(objectname)"])
            .lines()
            .map(str::to_owned)
            .chain(["0123456789abcdef0123456789abcdef01234567".to_owned()])
            .collect();
        let first = git(&sender, &["rev-parse", "HEAD^"]);
        commit(&sender, "c3.txt", "3\n");
        write(&sender.join("a.txt"), "changed\n");
        snapshot(&sender, "l2").await.unwrap();
        let file = dir.path().join("l2.bundle");

        let bundled = outgoing(&sender, "l2", &has, &file).await.unwrap();

        assert_eq!(bundled, Bundled::Written);
        // The receiver had the first commit: the bundle starts past it, the unknown id left out.
        assert_eq!(prerequisites(&file), std::slice::from_ref(&first));
        let before = objects(&receiver);
        let received = receive(&receiver, &file, "l2").await.unwrap();
        assert_ne!(objects(&receiver), before);
        assert_eq!(
            git(
                &receiver,
                &["rev-list", "--count", &format!("{first}..{}", received.tip)]
            ),
            "2"
        );

        // The other side has the snapshot already: nothing new, no file.
        let again = dir.path().join("again.bundle");
        let nothing = outgoing(&sender, "l2", &[received.snapshot], &again)
            .await
            .unwrap();
        assert_eq!(nothing, Bundled::NothingNew);
        assert!(!again.exists());
    }

    #[tokio::test]
    async fn a_repository_with_no_remote_travels_whole_the_first_time() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        commit(&sender, "c2.txt", "2\n");
        commit(&sender, "c3.txt", "3\n");
        assert_eq!(git(&sender, &["remote"]), "");
        snapshot(&sender, "l1").await.unwrap();
        let file = dir.path().join("l1.bundle");

        outgoing(&sender, "l1", &[], &file).await.unwrap();

        assert!(prerequisites(&file).is_empty());
        let receiver = dir.path().join("receiver");
        let received = clone(&receiver, &file, "l1").await.unwrap();
        assert_eq!(git(&receiver, &["rev-list", "--count", &received.tip]), "3");
        assert_eq!(received.tip, git(&sender, &["rev-parse", "HEAD"]));
        // Made from the bundle alone: it names no remote.
        assert_eq!(git(&receiver, &["remote"]), "");
    }

    #[tokio::test]
    async fn nothing_is_pushed_and_no_remote_is_contacted() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        // A remote that leaves a mark when git contacts it.
        let mark = dir.path().join("contacted");
        let url = format!("ext::sh -c touch% {}", mark.display());
        git(&sender, &["config", "protocol.ext.allow", "always"]);
        git(&sender, &["remote", "add", "origin", &url]);
        let _ = git_command(&sender, &["ls-remote", "origin"])
            .output()
            .unwrap();
        assert!(mark.exists(), "the remote leaves its mark");
        std::fs::remove_file(&mark).unwrap();
        let remotes = git(&sender, &["for-each-ref", "refs/remotes"]);
        write(&sender.join("a.txt"), "changed\n");
        let receiver = dir.path().join("receiver");
        let worktree = dir.path().join("workspace");

        let snapshot = travel(&sender, "l1", &receiver, &worktree).await;
        git(&worktree, &["commit", "--quiet", "--all", "-m", "work"]);
        let back = dir.path().join("back.bundle");
        returning(&receiver, &snapshot.branch, &[snapshot.commit], &back)
            .await
            .unwrap();
        fetch_returned(&sender, &back, "server", &snapshot.branch)
            .await
            .unwrap();

        assert!(!mark.exists(), "a remote was contacted");
        assert_eq!(git(&sender, &["for-each-ref", "refs/remotes"]), remotes);
        assert_eq!(git(&receiver, &["remote"]), "");
    }

    #[tokio::test]
    async fn the_received_branch_has_the_uncommitted_changes_uncommitted() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        write(&sender.join("a.txt"), "a, changed\n");
        std::fs::remove_file(sender.join("gone.txt")).unwrap();
        executable(&sender.join("b.txt"), true);
        executable(&sender.join("src/run.sh"), false);
        write(&sender.join("new/file.txt"), "new\n");
        let worktree = dir.path().join("workspace");

        let snapshot = travel(&sender, "l1", &dir.path().join("receiver"), &worktree).await;

        assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), snapshot.head);
        assert_eq!(
            git(&worktree, &["symbolic-ref", "--short", "HEAD"]),
            "feature"
        );
        // The same files, bytes and modes; changed, removed and unstaged as there, the new one
        // untracked.
        assert_eq!(files(&worktree), files(&sender));
        assert!(is_executable(&worktree.join("b.txt")));
        assert!(!is_executable(&worktree.join("src/run.sh")));
        assert!(!worktree.join("gone.txt").exists());
        assert_eq!(status(&worktree), status(&sender));
        let status = status(&worktree);
        assert!(status.contains("? new/file.txt"), "{status}");
        assert!(
            status
                .lines()
                .any(|l| l.starts_with("1 .D") && l.ends_with(" gone.txt"))
        );
        assert!(
            status
                .lines()
                .any(|l| l.starts_with("1 .M") && l.ends_with(" b.txt"))
        );
        // Nothing staged.
        git(&worktree, &["diff", "--cached", "--quiet"]);
    }

    #[tokio::test]
    async fn a_branch_name_taken_there_gets_a_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        let receiver = dir.path().join("receiver");
        travel(&sender, "l1", &receiver, &dir.path().join("w1")).await;
        assert_eq!(
            free_branch(&receiver, "feature").await.unwrap(),
            "feature-2"
        );
        assert_eq!(free_branch(&receiver, "other").await.unwrap(), "other");

        write(&sender.join("a.txt"), "second\n");
        snapshot(&sender, "l2").await.unwrap();
        let file = dir.path().join("l2.bundle");
        outgoing(&sender, "l2", &[], &file).await.unwrap();
        let received = receive(&receiver, &file, "l2").await.unwrap();
        let w2 = dir.path().join("w2");
        let second = make_workspace(&receiver, "feature", &received, &w2).await;

        assert_eq!(second, "feature-2");
        assert_eq!(git(&w2, &["symbolic-ref", "--short", "HEAD"]), "feature-2");
        assert_eq!(
            free_branch(&receiver, "feature").await.unwrap(),
            "feature-3"
        );
        assert!(free_branch(&receiver, "../heads/main").await.is_err());
    }

    #[tokio::test]
    async fn a_corrupt_bundle_or_one_missing_its_prerequisites_is_refused_and_fetches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        let receiver = dir.path().join("receiver");
        travel(&sender, "l1", &receiver, &dir.path().join("w1")).await;
        commit(&sender, "c2.txt", "2\n".repeat(4096).as_str());
        snapshot(&sender, "l2").await.unwrap();
        let tip = git(&receiver, &["rev-parse", &incoming_ref("l1")]);
        let whole = dir.path().join("whole.bundle");
        outgoing(&sender, "l2", &[], &whole).await.unwrap();
        let past = dir.path().join("past.bundle");
        outgoing(&sender, "l2", &[tip], &past).await.unwrap();
        // A repository that has none of the commits the second bundle starts past.
        let stranger = dir.path().join("stranger");
        std::fs::create_dir(&stranger).unwrap();
        git(&stranger, &["init", "--quiet"]);
        let bytes = std::fs::read(&whole).unwrap();
        let truncated = dir.path().join("truncated.bundle");
        std::fs::write(&truncated, &bytes[..bytes.len() - 40]).unwrap();
        let mut flipped = bytes.clone();
        let middle = flipped.len() - 100;
        flipped[middle] ^= 0xff;
        let corrupt = dir.path().join("corrupt.bundle");
        std::fs::write(&corrupt, flipped).unwrap();
        let garbage = dir.path().join("garbage.bundle");
        std::fs::write(&garbage, "not a bundle\n").unwrap();

        for (repo, file) in [
            (&stranger, &past),
            (&receiver, &truncated),
            (&receiver, &corrupt),
            (&receiver, &garbage),
        ] {
            let (refs_before, objects_before) = (refs(repo), objects(repo));

            let refused = receive(repo, file, "l2").await;

            assert!(refused.is_err(), "{} was taken", file.display());
            assert_eq!(refs(repo), refs_before, "{}", file.display());
            assert_eq!(objects(repo), objects_before, "{}", file.display());
        }
        let new = dir.path().join("new");
        assert!(clone(&new, &past, "l2").await.is_err());
        assert!(!new.exists(), "a refused clone leaves no folder");
        assert!(verify(&stranger, &past).await.is_err());
    }

    /// A launch sent and worked on: two commits there, on the received branch.
    async fn worked_on(dir: &Path) -> (PathBuf, PathBuf, Snapshot, String) {
        let sender = repository(dir);
        write(&sender.join("a.txt"), "changed\n");
        let receiver = dir.join("receiver");
        let worktree = dir.join("workspace");
        let snapshot = travel(&sender, "l1", &receiver, &worktree).await;
        git(&worktree, &["commit", "--quiet", "--all", "-m", "work"]);
        commit(&worktree, "more.txt", "more\n");
        let tip = git(&worktree, &["rev-parse", "HEAD"]);
        (sender, receiver, snapshot, tip)
    }

    #[tokio::test]
    async fn only_the_new_commits_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let (sender, receiver, snapshot, tip) = worked_on(dir.path()).await;
        let back = dir.path().join("back.bundle");

        let bundled = returning(
            &receiver,
            &snapshot.branch,
            std::slice::from_ref(&snapshot.commit),
            &back,
        )
        .await
        .unwrap();

        assert_eq!(bundled, Bundled::Written);
        assert_eq!(prerequisites(&back), std::slice::from_ref(&snapshot.head));
        let landed = fetch_returned(&sender, &back, "server", "feature")
            .await
            .unwrap();
        assert_eq!(git(&sender, &["rev-parse", &landed]), tip);
        let range = format!("{}..{landed}", snapshot.head);
        assert_eq!(git(&sender, &["rev-list", "--count", &range]), "2");

        // Nothing committed since: nothing comes back.
        let again = dir.path().join("again.bundle");
        let nothing = returning(&receiver, &snapshot.branch, &[tip], &again)
            .await
            .unwrap();
        assert_eq!(nothing, Bundled::NothingNew);
        assert!(!again.exists());
    }

    #[tokio::test]
    async fn returned_work_lands_in_the_machines_ref_never_on_the_user_s_branch() {
        let dir = tempfile::tempdir().unwrap();
        let (sender, receiver, snapshot, tip) = worked_on(dir.path()).await;
        let back = dir.path().join("back.bundle");
        returning(
            &receiver,
            &snapshot.branch,
            std::slice::from_ref(&snapshot.commit),
            &back,
        )
        .await
        .unwrap();
        let before = user_state(&sender);
        let heads = git(&sender, &["for-each-ref", "refs/heads"]);

        for (machine, branch) in [
            ("..", "feature"),
            ("../heads", "feature"),
            ("a/b", "feature"),
            ("", "feature"),
            ("server", "../heads/main"),
            ("server", "-x"),
            ("server", "a..b"),
            ("server", "a:b"),
        ] {
            let refused = fetch_returned(&sender, &back, machine, branch).await;
            assert!(refused.is_err(), "{machine} {branch} was taken");
        }
        assert_eq!(
            git(&sender, &["for-each-ref", "refs/flottille/machines"]),
            ""
        );

        let landed = fetch_returned(&sender, &back, "server", "feature")
            .await
            .unwrap();

        assert_eq!(landed, "refs/flottille/machines/server/feature");
        assert_eq!(git(&sender, &["rev-parse", &landed]), tip);
        assert_eq!(git(&sender, &["for-each-ref", "refs/heads"]), heads);
        assert_eq!(user_state(&sender), before);
    }

    #[tokio::test]
    async fn a_partial_clone_is_refused_and_its_remote_never_contacted() {
        let dir = tempfile::tempdir().unwrap();
        let server = repository(dir.path());
        commit(&server, "old.txt", "old\n");
        git(&server, &["rm", "--quiet", "old.txt"]);
        git(&server, &["commit", "--quiet", "-m", "no old"]);
        git(&server, &["config", "uploadpack.allowFilter", "true"]);
        let url = format!("file://{}", server.display());
        git(
            dir.path(),
            &["clone", "--quiet", "--filter=blob:none", &url, "partial"],
        );
        let partial = dir.path().join("partial");
        // Its remote leaves a mark when git contacts it: a blob it never fetched does.
        let mark = dir.path().join("contacted");
        git(&partial, &["config", "protocol.ext.allow", "always"]);
        let remote = format!("ext::sh -c touch% {}", mark.display());
        git(&partial, &["remote", "set-url", "origin", &remote]);
        let missing = git(&partial, &["rev-parse", "HEAD~1:old.txt"]);
        let _ = git_command(&partial, &["cat-file", "-p", &missing])
            .output()
            .unwrap();
        assert!(mark.exists(), "the remote leaves its mark");
        std::fs::remove_file(&mark).unwrap();
        write(&partial.join("a.txt"), "changed\n");
        // Ids the other machine names, one of them only the remote has.
        let has = [
            missing,
            "0123456789abcdef0123456789abcdef01234567".to_owned(),
        ];
        let file = dir.path().join("l1.bundle");

        let snapshot = snapshot(&partial, "l1").await;
        let outgoing = outgoing(&partial, "l1", &has, &file).await;
        let returning = returning(&partial, "feature", &has, &file).await;

        let refused = snapshot.expect_err("a snapshot of a partial clone is refused");
        assert!(refused.contains("partial clone"), "{refused}");
        assert!(outgoing.is_err() && returning.is_err());
        assert!(!mark.exists(), "the remote was contacted");
        assert!(!file.exists());
        assert_eq!(git(&partial, &["for-each-ref", "refs/flottille"]), "");
    }

    #[tokio::test]
    async fn the_receiver_s_lfs_smudge_downloads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        let pointer = "version https://git-lfs.github.com/spec/v1\noid sha256:0\nsize 1\n";
        write(&sender.join("model.bin"), pointer);
        snapshot(&sender, "l1").await.unwrap();
        let file = dir.path().join("l1.bundle");
        outgoing(&sender, "l1", &[], &file).await.unwrap();
        let receiver = dir.path().join("receiver");
        let received = clone(&receiver, &file, "l1").await.unwrap();
        // git-lfs as the user's configuration installs it: its smudge downloads the object
        // unless it is told to skip.
        let mark = dir.path().join("downloaded");
        let smudge = format!(
            "test -n \"$GIT_LFS_SKIP_SMUDGE\" || touch {}; cat",
            mark.display()
        );
        git(&receiver, &["config", "filter.lfs.smudge", &smudge]);
        write(&receiver.join(".git/info/attributes"), "*.bin filter=lfs\n");
        let worktree = dir.path().join("workspace");

        make_workspace(&receiver, "feature", &received, &worktree).await;

        assert!(!mark.exists(), "the smudge downloaded");
        assert_eq!(
            std::fs::read_to_string(worktree.join("model.bin")).unwrap(),
            pointer
        );
    }

    #[tokio::test]
    async fn a_tag_with_the_branch_s_name_leaves_the_branch_s_name() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        git(&sender, &["tag", "feature"]);

        let made = snapshot(&sender, "l1").await.unwrap();

        assert_eq!(made.branch, "feature");
    }

    #[tokio::test]
    async fn a_workspace_that_is_not_clean_is_refused_and_nothing_is_laid() {
        let dir = tempfile::tempdir().unwrap();
        let sender = repository(dir.path());
        write(&sender.join("a.txt"), "changed\n");
        snapshot(&sender, "l1").await.unwrap();
        let file = dir.path().join("l1.bundle");
        outgoing(&sender, "l1", &[], &file).await.unwrap();
        let receiver = dir.path().join("receiver");
        let received = clone(&receiver, &file, "l1").await.unwrap();
        let worktree = dir.path().join("workspace");
        let target = worktree.to_str().unwrap();
        git(
            &receiver,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "feature",
                target,
                &received.tip,
            ],
        );
        write(&worktree.join("left.txt"), "left by a setup\n");
        let before = (files(&worktree), status(&worktree));

        let refused = lay_snapshot(&worktree, &received.snapshot).await;

        assert!(refused.is_err(), "a workspace with changes was laid on");
        assert_eq!((files(&worktree), status(&worktree)), before);
        assert_eq!(
            std::fs::read_to_string(worktree.join("a.txt")).unwrap(),
            "a\n"
        );
    }
}
