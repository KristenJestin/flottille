//! The scenarios of workspaces, asked of the daemon over its socket, with throwaway repositories,
//! local bare remotes and temporary XDG folders.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::User;
use serde_json::{Value, json};

/// A project folder `work/app` holding `web`, `api` and `lib`, each pushed to its own bare remote
/// under `remotes/`. `web` and `api` have a `develop` branch, `lib` only `main`; every base rule is
/// `[develop, main]`. When `root` is set, the folder is a repository too, ignoring the others.
struct Fixture {
    user: User,
    folder: PathBuf,
}

const REPOSITORIES: [(&str, bool); 3] = [("web", true), ("api", true), ("lib", false)];

impl Fixture {
    fn new(root: bool) -> Fixture {
        Fixture::of(User::new(), root)
    }

    fn of(user: User, root: bool) -> Fixture {
        let folder = user.path("work/app");
        std::fs::create_dir_all(&folder).unwrap();
        if root {
            user.git_init(&folder);
            std::fs::write(folder.join(".gitignore"), "/web/\n/api/\n/lib/\n").unwrap();
            std::fs::write(folder.join("README.md"), "the root\n").unwrap();
            user.git(&folder, &["add", "."]);
            user.git(&folder, &["commit", "--quiet", "-m", "root"]);
            fixture_remote(&user, &folder, "root");
        }
        for (name, develop) in REPOSITORIES {
            let repository = folder.join(name);
            user.git_init(&repository);
            std::fs::write(repository.join(".gitignore"), ".env*\n").unwrap();
            std::fs::write(repository.join("README.md"), format!("{name}\n")).unwrap();
            user.git(&repository, &["add", "."]);
            user.git(&repository, &["commit", "--quiet", "-m", "start"]);
            fixture_remote(&user, &repository, name);
            if develop {
                user.git(&repository, &["checkout", "--quiet", "-b", "develop"]);
                std::fs::write(repository.join("DEVELOP.md"), "on develop\n").unwrap();
                user.git(&repository, &["add", "."]);
                user.git(&repository, &["commit", "--quiet", "-m", "on develop"]);
                user.git(&repository, &["push", "--quiet", "origin", "develop"]);
                user.git(&repository, &["checkout", "--quiet", "main"]);
            }
        }
        Fixture { user, folder }
    }

    /// Writes `projects/app.toml`: the repositories (with the root one first when there is one),
    /// then `rest` (variables, setup…).
    fn configure(&self, rest: &str) {
        let mut text = format!(
            "folder = \"{}\"\nworkspaces_root = \"{}\"\n",
            self.folder.display(),
            self.root().display()
        );
        if self.folder.join(".git").exists() {
            text.push_str("[[repositories]]\npath = \".\"\n");
        }
        for (name, _) in REPOSITORIES {
            text.push_str(&format!(
                "[[repositories]]\npath = \"{name}\"\nbase = [\"develop\", \"main\"]\n"
            ));
        }
        text.push_str(rest);
        self.user.write_project_file("app", &text);
    }

    fn root(&self) -> PathBuf {
        self.user.path("work/app-work")
    }

    fn workspace(&self, name: &str) -> PathBuf {
        self.root().join(name)
    }

    fn remote(&self, name: &str) -> PathBuf {
        self.user.path(&format!("remotes/{name}.git"))
    }

    /// A folder for the scripts the run steps call, outside every repository.
    fn script(&self, name: &str, text: &str) -> PathBuf {
        let path = self.user.path(&format!("tools/{name}"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn create(&self, params: Value) -> Result<Value, (String, String)> {
        let mut asked = json!({"project": "app", "name": "login"});
        asked
            .as_object_mut()
            .unwrap()
            .extend(params.as_object().unwrap().clone());
        self.user.call("workspace.create", asked)
    }

    fn show(&self, name: &str) -> Result<Value, (String, String)> {
        self.user
            .call("workspace.show", json!({"project": "app", "name": name}))
    }

    /// The workspace once its preparation stopped, ready or failed.
    fn settled(&self, name: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let workspace = self.show(name).unwrap();
            if workspace["state"] != "preparing" {
                return workspace;
            }
            assert!(Instant::now() < deadline, "still preparing: {workspace}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn remove(&self, name: &str) -> Result<Value, (String, String)> {
        self.user
            .call("workspace.remove", json!({"project": "app", "name": name}))
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        self.user.git_output(dir, args).trim().to_owned()
    }
}

/// Pushes `repository`'s `main` to a new bare remote `remotes/<name>.git`, as `origin`.
fn fixture_remote(user: &User, repository: &Path, name: &str) {
    let remote = user.path(&format!("remotes/{name}.git"));
    std::fs::create_dir_all(&remote).unwrap();
    user.git(&remote, &["init", "--quiet", "--bare"]);
    user.git(
        repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(repository, &["push", "--quiet", "origin", "main"]);
}

fn step_states(workspace: &Value) -> Vec<String> {
    workspace["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            format!(
                "{} {}",
                step["kind"].as_str().unwrap(),
                step["state"].as_str().unwrap()
            )
        })
        .collect()
}

#[test]
fn a_shown_workspace_counts_the_lines_added_and_removed_in_each_repository() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    let web = fixture.workspace("login").join("web");
    let lib = fixture.workspace("login").join("lib");
    // Committed on the workspace's branch, changed and not committed, and new and not tracked.
    std::fs::remove_file(web.join("DEVELOP.md")).unwrap();
    fixture.git(&web, &["commit", "--quiet", "-am", "drop develop's note"]);
    std::fs::write(web.join("README.md"), "web\nmore\nlines\n").unwrap();
    std::fs::write(web.join("NOTES.md"), "one\ntwo\nthree").unwrap();
    std::fs::write(web.join(".env"), "IGNORED=1\nIGNORED=2\n").unwrap();
    std::fs::remove_file(lib.join("README.md")).unwrap();

    let shown = fixture.show("login").unwrap();

    let changes: Vec<(&str, &Value)> = shown["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|repository| (repository["path"].as_str().unwrap(), &repository["changes"]))
        .collect();
    assert_eq!(
        changes,
        [
            ("web", &json!({"added": 5, "removed": 1, "capped": false})),
            ("api", &json!({"added": 0, "removed": 0, "capped": false})),
            ("lib", &json!({"added": 0, "removed": 1, "capped": false})),
        ],
        "against the base, committed or not, new files counted, ignored ones not"
    );
    let listed = fixture.user.call("workspace.list", json!({})).unwrap();
    assert!(
        listed["workspaces"][0]["repositories"][0]
            .get("changes")
            .is_none(),
        "a listing runs no git"
    );
}

#[test]
fn a_shown_workspace_says_when_its_preparation_ended() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");

    let shown = fixture.show("login").unwrap();

    let watch = fixture.user.watch(&["--after", "0"]);
    let ready = loop {
        let event = watch.next_event();
        if event["name"] == "workspace.ready" {
            break event;
        }
    };
    assert_eq!(shown["ready_at"], ready["at"], "when it was ready");
    assert!(shown["ready_at"].as_u64().unwrap() >= shown["created_at"].as_u64().unwrap());
    let listed = fixture.user.call("workspace.list", json!({})).unwrap();
    assert!(
        listed["workspaces"][0].get("ready_at").is_none(),
        "the listing reads no events"
    );
}

#[test]
fn a_workspace_never_prepared_has_no_end_of_preparation() {
    let fixture = Fixture::new(false);
    let script = fixture.script("fail.sh", "exit 3\n");
    fixture.configure(&format!("[[setup]]\nrun = \"sh {}\"\n", script.display()));
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "failed");

    assert_eq!(fixture.show("login").unwrap()["ready_at"], json!(null));
}

#[test]
fn showing_a_workspace_does_not_write_its_index() {
    use std::os::unix::fs::MetadataExt as _;
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    let web = fixture.workspace("login").join("web");
    let index = web.join(fixture.git(&web, &["rev-parse", "--git-path", "index"]));
    let before = std::fs::metadata(&index).unwrap();
    // The same content, a newer time: a refreshing git rewrites the index for it.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(web.join("README.md"), "web\n").unwrap();

    fixture.show("login").unwrap();

    let after = std::fs::metadata(&index).unwrap();
    assert_eq!(
        (after.ino(), after.mtime(), after.mtime_nsec()),
        (before.ino(), before.mtime(), before.mtime_nsec()),
        "an agent's own git add or commit could fail on the index being rewritten"
    );
}

#[test]
fn the_new_files_of_a_workspace_are_read_without_following_links_up_to_a_limit() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    let web = fixture.workspace("login").join("web");
    let api = fixture.workspace("login").join("api");
    let outside = fixture.user.path("outside.txt");
    std::fs::write(&outside, "far\naway\n").unwrap();
    std::os::unix::fs::symlink(&outside, web.join("LINK.md")).unwrap();
    let fifo = web.join("pipe");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(web.join("NOTES.md"), "one\ntwo\n").unwrap();
    // More new files than are read: the count says it stopped.
    let many = api.join("generated");
    std::fs::create_dir_all(&many).unwrap();
    for at in 0..1001 {
        std::fs::write(many.join(format!("{at}.txt")), "x\n").unwrap();
    }

    let shown = fixture.show("login").unwrap();

    assert_eq!(
        shown["repositories"][0]["changes"],
        json!({"added": 2, "removed": 0, "capped": false}),
        "a link and a pipe count no line, and the pipe is not waited on"
    );
    assert_eq!(
        shown["repositories"][1]["changes"],
        json!({"added": 1000, "removed": 0, "capped": true})
    );
}

#[test]
fn a_workspace_over_three_repositories_takes_develop_where_it_exists_and_main_elsewhere() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    // Someone pushed to `web`'s develop since the main checkout last fetched: the base must be it.
    let other = fixture.user.path("other-web");
    fixture.user.git(
        fixture.user.path("").as_path(),
        &[
            "clone",
            "--quiet",
            "--branch",
            "develop",
            fixture.remote("web").to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join("NEW.md"), "new\n").unwrap();
    fixture.user.git(&other, &["add", "."]);
    fixture
        .user
        .git(&other, &["commit", "--quiet", "-m", "pushed by someone"]);
    fixture
        .user
        .git(&other, &["push", "--quiet", "origin", "develop"]);
    let pushed = fixture.git(&other, &["rev-parse", "HEAD"]);

    let created = fixture.create(json!({})).unwrap();

    assert_eq!(created["branch"], "app/login");
    assert_eq!(
        created["folder"],
        fixture.workspace("login").to_str().unwrap()
    );
    let bases: Vec<_> = created["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{} {}",
                r["path"].as_str().unwrap(),
                r["base"]["branch"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(bases, ["web develop", "api develop", "lib main"]);
    assert_eq!(created["repositories"][0]["base"]["commit"], pushed);
    assert_eq!(created["repositories"][0]["base"]["fetched_now"], true);
    let workspace = fixture.settled("login");
    assert_eq!(workspace["state"], "ready");
    for (name, develop) in REPOSITORIES {
        let worktree = fixture.workspace("login").join(name);
        assert_eq!(
            fixture.git(&worktree, &["branch", "--show-current"]),
            "app/login"
        );
        let base = if develop {
            "origin/develop"
        } else {
            "origin/main"
        };
        let main = fixture.folder.join(name);
        assert_eq!(
            fixture.git(&worktree, &["rev-parse", "HEAD"]),
            fixture.git(&main, &["rev-parse", base])
        );
        // The user's own checkout is not touched.
        assert_eq!(fixture.git(&main, &["branch", "--show-current"]), "main");
    }
    assert!(fixture.workspace("login").join("web/NEW.md").is_file());
    assert_eq!(
        step_states(&workspace),
        ["worktree done", "worktree done", "worktree done"]
    );
}

#[test]
fn every_step_is_an_event() {
    let fixture = Fixture::new(false);
    let script = fixture.script("ok.sh", "exit 0\n");
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {}\"\nrepository = \"lib\"\n",
        script.display()
    ));

    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    fixture.settled("login");

    let watch = fixture.user.watch(&["--after", "0"]);
    let mut names = Vec::new();
    while names.last().map(String::as_str) != Some("workspace.ready") {
        let event = watch.next_event();
        let name = event["name"].as_str().unwrap();
        if name.starts_with("workspace.") {
            assert_eq!(event["data"]["project"], "app", "{event}");
            assert_eq!(event["data"]["workspace"], "login", "{event}");
            names.push(name.to_owned());
        }
    }
    assert_eq!(
        names,
        [
            "workspace.created",
            "workspace.step_started",
            "workspace.step_done",
            "workspace.step_started",
            "workspace.step_done",
            "workspace.ready"
        ]
    );
}

/// Makes `name` a repository of this machine alone: its remote is gone, with its tracking refs,
/// and its `main` holds a commit no remote has. The commit of that `main`.
fn local_only(fixture: &Fixture, name: &str) -> String {
    let repository = fixture.folder.join(name);
    fixture
        .user
        .git(&repository, &["remote", "remove", "origin"]);
    std::fs::write(repository.join("LOCAL.md"), "only here\n").unwrap();
    fixture.user.git(&repository, &["add", "."]);
    fixture
        .user
        .git(&repository, &["commit", "--quiet", "-m", "only here"]);
    fixture.git(&repository, &["rev-parse", "HEAD"])
}

#[test]
fn a_workspace_of_a_repository_with_no_remote_is_created_from_the_local_branch() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    let local = local_only(&fixture, "lib");

    let created = fixture.create(json!({})).unwrap();

    let lib = &created["repositories"][2];
    assert_eq!(lib["path"], "lib");
    assert_eq!(lib["base"]["branch"], "main");
    assert_eq!(lib["base"]["ref"], "refs/heads/main");
    assert_eq!(lib["base"]["commit"], local.as_str());
    assert_eq!(lib["base"]["fetched_now"], false);
    assert_eq!(
        lib["base"]["note"],
        "no remote origin: started from the local main"
    );
    // The others still fetch, and say nothing.
    assert_eq!(created["repositories"][0]["base"]["fetched_now"], true);
    assert_eq!(created["repositories"][0]["base"].get("note"), None);
    assert_eq!(fixture.settled("login")["state"], "ready");
    let worktree = fixture.workspace("login").join("lib");
    assert_eq!(fixture.git(&worktree, &["rev-parse", "HEAD"]), local);
    assert!(worktree.join("LOCAL.md").is_file());
}

#[test]
fn a_failed_fetch_falls_back_the_same_way() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    // `origin` of lib now leads nowhere: its fetch fails.
    std::fs::rename(fixture.remote("lib"), fixture.user.path("remotes/gone.git")).unwrap();
    // Nothing tracked of it: the local branch is what is left.
    fixture.user.git(
        &fixture.folder.join("lib"),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    let local = fixture.git(&fixture.folder.join("lib"), &["rev-parse", "main"]);

    let created = fixture.create(json!({})).unwrap();

    let base = &created["repositories"][2]["base"];
    assert_eq!(base["branch"], "main");
    assert_eq!(base["ref"], "refs/heads/main");
    assert_eq!(base["commit"], local.as_str());
    assert_eq!(base["fetched_now"], false);
    let note = base["note"].as_str().unwrap();
    assert!(note.starts_with("origin could not be fetched ("), "{note}");
    assert!(note.ends_with("): started from the local main"), "{note}");
    assert!(!note.contains('\n'), "one line: {note}");
    assert_eq!(fixture.settled("login")["state"], "ready");
    assert_eq!(
        fixture.git(
            &fixture.workspace("login").join("lib"),
            &["rev-parse", "HEAD"]
        ),
        local
    );
}

#[test]
fn the_command_line_says_in_one_line_where_a_repository_without_remote_started() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    local_only(&fixture, "lib");

    let output = fixture.user.run(&["workspace", "create", "app", "login"]);

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("  lib: main, no remote origin: started from the local main\n"),
        "{text}"
    );
}

#[test]
fn a_workspace_created_with_two_of_three_repositories() {
    let fixture = Fixture::new(false);
    fixture.configure("");

    let created = fixture
        .create(json!({"repositories": ["web", "lib"], "bases": {"web": "main"}}))
        .unwrap();

    let bases: Vec<_> = created["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            format!(
                "{} {}",
                r["path"].as_str().unwrap(),
                r["base"]["branch"].as_str().unwrap()
            )
        })
        .collect();
    // `web` would take develop by its rule: the base chosen for it wins.
    assert_eq!(bases, ["web main", "lib main"]);
    assert_eq!(fixture.settled("login")["state"], "ready");
    assert!(fixture.workspace("login").join("web/.git").exists());
    assert!(!fixture.workspace("login").join("api").exists());
    assert!(!fixture.workspace("login").join("web/DEVELOP.md").exists());
}

#[test]
fn a_base_chosen_for_a_repository_the_workspace_does_not_take_is_refused() {
    let fixture = Fixture::new(false);
    fixture.configure("");

    let (code, message) = fixture
        .create(json!({"repositories": ["web"], "bases": {"api": "main"}}))
        .unwrap_err();

    assert_eq!(code, "refused");
    assert_eq!(
        message,
        "a base is chosen for api, which the workspace does not take"
    );
    assert!(!fixture.workspace("login").exists());
}

#[test]
fn the_per_repository_status() {
    let fixture = Fixture::new(false);
    let script = fixture.script("ok.sh", "exit 0\n");
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {}\"\nfor_each_repository = true\n",
        script.display()
    ));
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    let web = fixture.workspace("login").join("web");
    for (file, subject) in [("a.ts", "first"), ("b.ts", "second")] {
        std::fs::write(web.join(file), "export {}\n").unwrap();
        fixture.user.git(&web, &["add", "."]);
        fixture
            .user
            .git(&web, &["commit", "--quiet", "-m", subject]);
    }
    std::fs::write(web.join("README.md"), "changed\n").unwrap();
    std::fs::write(web.join("draft.ts"), "draft\n").unwrap();

    let shown = fixture.show("login").unwrap();

    let web = &shown["repositories"][0];
    assert_eq!(web["path"], "web");
    assert_eq!(web["branch"], "app/login");
    assert_eq!(web["ahead"], 2);
    assert_eq!(web["uncommitted"], 2);
    let lib = &shown["repositories"][2];
    assert_eq!(lib["branch"], "app/login");
    assert_eq!(lib["ahead"], 0);
    assert_eq!(lib["uncommitted"], 0);
}

#[test]
fn the_uncommitted_count_looks_inside_untracked_folders_and_skips_the_links_flottille_made() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    let web = fixture.workspace("login").join("web");
    // A link of Flottille's in a folder git does not know, and three files of the user's in another.
    std::fs::create_dir_all(web.join("cache")).unwrap();
    std::os::unix::fs::symlink(fixture.folder.join("api"), web.join("cache/api")).unwrap();
    std::fs::create_dir_all(web.join("drafts")).unwrap();
    for name in ["a.md", "b.md", "c.md"] {
        std::fs::write(web.join("drafts").join(name), "draft\n").unwrap();
    }

    let shown = fixture.show("login").unwrap();

    assert_eq!(shown["repositories"][0]["uncommitted"], 3);
}

/// A main checkout whose `api` holds two local files git does not know, copied by the setup.
fn fixture_copying_two_files() -> Fixture {
    let fixture = Fixture::new(false);
    let api = fixture.folder.join("api");
    std::fs::write(api.join("local-a.cfg"), "a=1\nb=2\nc=3\n").unwrap();
    std::fs::write(api.join("local-b.cfg"), "d=4\ne=5\n").unwrap();
    fixture
        .configure("[[setup]]\ncopy = [\"local-a.cfg\", \"local-b.cfg\"]\nrepository = \"api\"\n");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    fixture
}

#[test]
fn the_files_a_setup_copy_wrote_are_not_changes_of_the_workspace() {
    let fixture = fixture_copying_two_files();
    let api = fixture.workspace("login").join("api");
    assert!(api.join("local-a.cfg").is_file(), "the copy was made");

    let shown = fixture.show("login").unwrap();

    let api = &shown["repositories"][1];
    assert_eq!(api["path"], "api");
    assert_eq!(api["uncommitted"], 0, "{api}");
    assert_eq!(api["changes"]["added"], 0, "{api}");
}

#[test]
fn a_file_the_setup_copied_and_the_user_edited_is_a_change_again_and_so_is_a_file_of_their_own() {
    let fixture = fixture_copying_two_files();
    let api = fixture.workspace("login").join("api");
    std::fs::write(api.join("local-a.cfg"), "a=1\nb=2\nc=3\nf=6\n").unwrap();
    std::fs::write(api.join("draft.md"), "mine\n").unwrap();

    let shown = fixture.show("login").unwrap();

    let api = &shown["repositories"][1];
    assert_eq!(api["uncommitted"], 2, "{api}");
    assert_eq!(api["changes"]["added"], 5, "{api}");
}

#[test]
fn a_workspace_of_a_repository_with_no_remote_is_removed_unless_it_holds_commits_of_its_own() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    local_only(&fixture, "lib");
    fixture
        .create(json!({"name": "clean", "repositories": ["lib"]}))
        .unwrap();
    fixture
        .create(json!({"name": "work", "repositories": ["lib"]}))
        .unwrap();
    fixture.settled("clean");
    fixture.settled("work");
    let lib = fixture.workspace("work").join("lib");
    std::fs::write(lib.join("feature.rs"), "fn f() {}\n").unwrap();
    fixture.user.git(&lib, &["add", "."]);
    fixture
        .user
        .git(&lib, &["commit", "--quiet", "-m", "add the feature"]);
    let commit = fixture.git(&lib, &["rev-parse", "--short", "HEAD"]);

    let removed = fixture.remove("clean");
    let (code, message) = fixture.remove("work").unwrap_err();

    let removed = removed.unwrap();
    assert_eq!(removed["removed"], "clean");
    assert_eq!(
        removed["branches"],
        json!([{"repository": "lib", "deleted": true, "why": "it has no commit of its own"}])
    );
    assert_eq!(code, "refused");
    assert!(
        message.contains(&format!(
            "lib has commits on no other branch, on its branch app/work: {commit} add the \
             feature"
        )),
        "{message}"
    );
    assert!(lib.join("feature.rs").exists());
}

#[test]
fn a_failed_fetch_starts_from_the_tracking_ref_the_offline_creation_would_take() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    // `lib` has `origin/develop` as last fetched, and no `develop` of its own.
    let lib = fixture.folder.join("lib");
    fixture
        .user
        .git(&lib, &["checkout", "--quiet", "-b", "develop"]);
    std::fs::write(lib.join("DEVELOP.md"), "on develop\n").unwrap();
    fixture.user.git(&lib, &["add", "."]);
    fixture
        .user
        .git(&lib, &["commit", "--quiet", "-m", "on develop"]);
    fixture
        .user
        .git(&lib, &["push", "--quiet", "origin", "develop"]);
    fixture.user.git(&lib, &["checkout", "--quiet", "main"]);
    fixture
        .user
        .git(&lib, &["branch", "--quiet", "-D", "develop"]);
    let tracked = fixture.git(&lib, &["rev-parse", "refs/remotes/origin/develop"]);
    std::fs::rename(fixture.remote("lib"), fixture.user.path("remotes/gone.git")).unwrap();

    let created = fixture.create(json!({"repositories": ["lib"]})).unwrap();

    let base = &created["repositories"][0]["base"];
    assert_eq!(base["branch"], "develop");
    assert_eq!(base["ref"], "refs/remotes/origin/develop");
    assert_eq!(base["commit"], tracked.as_str());
    assert_eq!(base["fetched_now"], false);
    let note = base["note"].as_str().unwrap();
    assert!(note.starts_with("origin could not be fetched ("), "{note}");
    assert!(
        note.ends_with("): started from origin/develop as last fetched"),
        "{note}"
    );
    assert_eq!(fixture.settled("login")["state"], "ready");
    assert!(fixture.workspace("login").join("lib/DEVELOP.md").is_file());
}

/// A commit of `lib`'s own `main`, kept local.
fn local_commit(fixture: &Fixture) {
    let lib = fixture.folder.join("lib");
    std::fs::write(lib.join("LOCAL.md"), "only here\n").unwrap();
    fixture.user.git(&lib, &["add", "."]);
    fixture
        .user
        .git(&lib, &["commit", "--quiet", "-m", "only here"]);
}

#[test]
fn a_workspace_started_from_a_local_base_is_removed_when_the_fetch_failed() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    std::fs::rename(fixture.remote("lib"), fixture.user.path("remotes/gone.git")).unwrap();
    fixture.user.git(
        &fixture.folder.join("lib"),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    local_commit(&fixture);
    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");

    let removed = fixture.remove("login");

    assert_eq!(removed.unwrap()["removed"], "login");
    assert!(!fixture.workspace("login").exists());
}

#[test]
fn a_workspace_started_from_a_local_base_is_removed_when_the_remote_is_empty() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    // `origin` of lib leads to a bare repository where `main` is not.
    let empty = fixture.user.path("remotes/empty.git");
    std::fs::create_dir_all(&empty).unwrap();
    fixture.user.git(&empty, &["init", "--quiet", "--bare"]);
    fixture.user.git(
        &fixture.folder.join("lib"),
        &["remote", "set-url", "origin", empty.to_str().unwrap()],
    );
    fixture.user.git(
        &fixture.folder.join("lib"),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    let workspace = fixture.settled("login");
    assert_eq!(
        workspace["repositories"][0]["base"]["ref"],
        "refs/heads/main"
    );

    let removed = fixture.remove("login");

    assert_eq!(removed.unwrap()["removed"], "login");
}

#[test]
fn a_workspace_started_from_a_local_base_still_refuses_its_own_commits() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    std::fs::rename(fixture.remote("lib"), fixture.user.path("remotes/gone.git")).unwrap();
    fixture.user.git(
        &fixture.folder.join("lib"),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    local_commit(&fixture);
    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    fixture.settled("login");
    let lib = fixture.workspace("login").join("lib");
    std::fs::write(lib.join("feature.rs"), "fn f() {}\n").unwrap();
    fixture.user.git(&lib, &["add", "."]);
    fixture
        .user
        .git(&lib, &["commit", "--quiet", "-m", "add the feature"]);
    let commit = fixture.git(&lib, &["rev-parse", "--short", "HEAD"]);

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains(&format!("{commit} add the feature")),
        "{message}"
    );
    assert!(!message.contains("only here"), "{message}");
}

#[test]
fn offline_the_creation_proceeds_on_the_last_tracking_refs_and_says_their_age() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    std::fs::rename(fixture.remote("lib"), fixture.user.path("remotes/gone.git")).unwrap();

    let output = fixture
        .user
        .run(&["workspace", "create", "app", "login", "--offline"]);

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("lib: main, offline: the tracking ref as of "),
        "{text}"
    );
    assert!(text.contains(" ago"), "{text}");
    let workspace = fixture.settled("login");
    let lib = &workspace["repositories"][2]["base"];
    assert_eq!(lib["fetched_now"], false);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let at = lib["fetched_at"].as_u64().unwrap();
    assert!(at <= now && now - at < 600_000, "{lib}");
    assert_eq!(workspace["state"], "ready");
}

#[test]
fn a_program_set_in_the_git_config_of_a_repository_is_never_run_by_the_daemon() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new(false);
    fixture.configure("");
    let marker = fixture.user.path("ran");
    let program = fixture.script(
        "fsmonitor",
        &format!("#!/bin/sh\ntouch {}\n", marker.display()),
    );
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let hook = fixture.folder.join("lib/.git/hooks/post-checkout");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    for repository in ["web", "api", "lib"] {
        fixture.user.git(
            &fixture.folder.join(repository),
            &["config", "core.fsmonitor", program.to_str().unwrap()],
        );
    }

    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    // A change to look at: status and the diff both read the index.
    std::fs::write(fixture.workspace("login").join("lib/new.rs"), "fn f() {}\n").unwrap();
    fixture.show("login").unwrap();
    fixture.remove("login").ok();

    assert!(!marker.exists(), "a program of the git config was run");
}

#[test]
fn a_branch_name_git_refuses_is_refused_before_anything_is_written() {
    let fixture = Fixture::new(false);
    fixture.configure("");

    for branch in ["release.lock/1", "login@{1}", "feature/.hidden"] {
        let (code, message) = fixture.create(json!({"branch": branch})).unwrap_err();

        assert_eq!(code, "refused");
        assert_eq!(
            message,
            format!("{branch} is not a branch name git accepts")
        );
    }
    assert!(!fixture.workspace("login").exists());
    let listed = fixture.user.call("workspace.list", json!({})).unwrap();
    assert_eq!(listed["workspaces"], json!([]));
}

#[test]
fn a_copy_never_overwrites_a_file_already_there() {
    let fixture = Fixture::new(false);
    let api = fixture.folder.join("api");
    std::fs::write(api.join("config.json"), "committed\n").unwrap();
    fixture.user.git(&api, &["add", "config.json"]);
    fixture
        .user
        .git(&api, &["commit", "--quiet", "-m", "config"]);
    fixture
        .user
        .git(&api, &["push", "--quiet", "origin", "main"]);
    // The develop branch is the base: it gets the file too.
    fixture.user.git(&api, &["checkout", "--quiet", "develop"]);
    fixture.user.git(&api, &["merge", "--quiet", "main"]);
    fixture
        .user
        .git(&api, &["push", "--quiet", "origin", "develop"]);
    fixture.user.git(&api, &["checkout", "--quiet", "main"]);
    std::fs::write(api.join("config.json"), "changed in the main checkout\n").unwrap();
    std::fs::write(api.join(".env"), "SECRET_PLACE=main\n").unwrap();
    std::fs::write(api.join(".env.local"), "LOCAL=1\n").unwrap();
    std::fs::create_dir(api.join(".env.d")).unwrap();
    std::fs::write(api.join(".env.d/extra"), "EXTRA=1\n").unwrap();
    fixture.configure(
        "[[setup]]\ncopy = [\".env\", \".env.*\", \"config.json\", \".env.missing\"]\nrepository = \"api\"\n",
    );

    fixture.create(json!({})).unwrap();
    let workspace = fixture.settled("login");

    assert_eq!(workspace["state"], "ready", "{workspace}");
    let api = fixture.workspace("login").join("api");
    assert_eq!(
        std::fs::read_to_string(api.join(".env")).unwrap(),
        "SECRET_PLACE=main\n"
    );
    assert_eq!(
        std::fs::read_to_string(api.join(".env.local")).unwrap(),
        "LOCAL=1\n"
    );
    assert_eq!(
        std::fs::read_to_string(api.join(".env.d/extra")).unwrap(),
        "EXTRA=1\n"
    );
    assert_eq!(
        std::fs::read_to_string(api.join("config.json")).unwrap(),
        "committed\n"
    );
}

#[test]
fn root_entries_never_links_git_nor_gitignore() {
    let fixture = Fixture::new(true);
    std::fs::create_dir(fixture.folder.join("notes")).unwrap();
    std::fs::write(fixture.folder.join("notes/plan.md"), "plan\n").unwrap();
    std::fs::write(fixture.folder.join("AGENTS.local.md"), "local rules\n").unwrap();
    fixture.configure("[[setup]]\nroot_entries = true\n");

    // Without the root repository the workspace folder is a plain folder: `.git` and `.gitignore`
    // are not there, and linking them would make it look like the root's checkout.
    fixture
        .create(json!({"repositories": ["web", "api"]}))
        .unwrap();
    let workspace = fixture.settled("login");

    assert_eq!(workspace["state"], "ready", "{workspace}");
    let folder = fixture.workspace("login");
    let link = |name: &str| std::fs::read_link(folder.join(name)).ok();
    assert_eq!(link("notes"), Some(fixture.folder.join("notes")));
    assert_eq!(
        link("AGENTS.local.md"),
        Some(fixture.folder.join("AGENTS.local.md"))
    );
    assert_eq!(link("README.md"), Some(fixture.folder.join("README.md")));
    assert!(std::fs::symlink_metadata(folder.join(".git")).is_err());
    assert!(std::fs::symlink_metadata(folder.join(".gitignore")).is_err());
    // A repository is never linked, taken or not.
    assert!(
        std::fs::symlink_metadata(folder.join("web"))
            .unwrap()
            .is_dir()
    );
    assert!(std::fs::symlink_metadata(folder.join("lib")).is_err());
}

#[test]
fn run_steps_for_each_repository_run_in_dependency_order() {
    let fixture = Fixture::new(false);
    let order = fixture.user.path("order.txt");
    let script = fixture.script("record.sh", "echo \"$1 $(pwd)\" >> \"$2\"\n");
    // In the file, `web` comes first; it depends on both others, and `api` on `lib`.
    let mut text = format!(
        "folder = \"{}\"\nworkspaces_root = \"{}\"\n",
        fixture.folder.display(),
        fixture.root().display()
    );
    text.push_str(
        "[[repositories]]\npath = \"web\"\nbase = [\"develop\"]\ndepends_on = [\"api\", \"lib\"]\n",
    );
    text.push_str(
        "[[repositories]]\npath = \"api\"\nbase = [\"develop\"]\ndepends_on = [\"lib\"]\n",
    );
    text.push_str("[[repositories]]\npath = \"lib\"\n");
    text.push_str(&format!(
        "[[setup]]\nrun = \"sh {} {{repository}} {}\"\nfor_each_repository = true\n",
        script.display(),
        order.display()
    ));
    fixture.user.write_project_file("app", &text);

    fixture.create(json!({})).unwrap();
    let workspace = fixture.settled("login");

    assert_eq!(workspace["state"], "ready", "{workspace}");
    let folder = fixture.workspace("login");
    let canonical = |name: &str| folder.join(name).canonicalize().unwrap();
    assert_eq!(
        std::fs::read_to_string(order).unwrap(),
        format!(
            "lib {}\napi {}\nweb {}\n",
            canonical("lib").display(),
            canonical("api").display(),
            canonical("web").display()
        )
    );
}

#[test]
fn a_run_failing_stops_the_preparation_with_its_last_lines_kept() {
    let fixture = Fixture::new(false);
    let failing = fixture.script(
        "fail.sh",
        "i=1\nwhile [ $i -le 50 ]; do echo \"line $i\"; i=$((i+1)); done\nexit 3\n",
    );
    let after = fixture.script("after.sh", "touch \"$1\"\n");
    let ran = fixture.user.path("after-ran");
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {}\"\nrepository = \"api\"\n[[setup]]\nrun = \"sh {} {}\"\n",
        failing.display(),
        after.display(),
        ran.display()
    ));

    fixture.create(json!({})).unwrap();
    let workspace = fixture.settled("login");

    assert_eq!(workspace["state"], "failed");
    let steps = workspace["steps"].as_array().unwrap();
    let failed = &steps[3];
    assert_eq!(failed["state"], "failed");
    assert_eq!(
        failed["failure"]["doing"],
        format!("sh {} (in api)", failing.display())
    );
    let kept: Vec<String> = (11..=50).map(|i| format!("line {i}")).collect();
    assert_eq!(failed["failure"]["output"], kept.join("\n"));
    assert_eq!(steps[4]["state"], "pending");
    assert!(!ran.exists());
    // The worktrees made before it are kept.
    assert!(fixture.workspace("login").join("api/.git").exists());
}

#[test]
fn a_run_step_leaving_a_child_in_the_background_ends_with_its_own_process() {
    let fixture = Fixture::new(false);
    let pid = fixture.user.path("pid");
    // The child holds the output pipe: the step must not wait for it, nor leave it running.
    let script = fixture.script(
        "background.sh",
        "sleep 60 &\necho $! > \"$1\"\necho started\n",
    );
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {} {}\"\nrepository = \"lib\"\n",
        script.display(),
        pid.display()
    ));

    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    let workspace = fixture.settled("login");

    assert_eq!(workspace["state"], "ready", "{workspace}");
    let pid = std::fs::read_to_string(&pid).unwrap().trim().to_owned();
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(&pid) {
        assert!(
            Instant::now() < deadline,
            "the background child {pid} still runs"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_run_step_past_its_time_limit_is_stopped_and_fails() {
    let fixture = Fixture::new(false);
    let script = fixture.script("slow.sh", "echo started\nsleep 60\n");
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {}\"\nrepository = \"lib\"\ntime_limit = 1\n",
        script.display()
    ));

    let started = Instant::now();
    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    let workspace = fixture.settled("login");

    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(workspace["state"], "failed", "{workspace}");
    assert_eq!(
        workspace["steps"][1]["failure"]["output"],
        "started\nit was stopped after 1 s"
    );
}

/// Whether a process runs, a zombie waiting to be reaped counting as ended.
fn alive(pid: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        // The state follows the command name, which is in parentheses.
        let state = stat.rsplit(')').next().unwrap_or("").trim_start();
        !state.starts_with(['Z', 'X'])
    })
}

#[test]
fn prepare_resumes_from_the_failed_step_and_never_reruns_a_finished_run() {
    let fixture = Fixture::new(false);
    let record = fixture.user.path("record.txt");
    let marker = fixture.user.path("fixed");
    let recorder = fixture.script("record.sh", "echo \"$1\" >> \"$2\"\n");
    let flaky = fixture.script(
        "flaky.sh",
        "echo flaky >> \"$2\"\nif [ -e \"$1\" ]; then exit 0; fi\necho 'not yet'\nexit 1\n",
    );
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {r} first {out}\"\n\
         [[setup]]\nrun = \"sh {f} {m} {out}\"\n\
         [[setup]]\nrun = \"sh {r} third {out}\"\n",
        r = recorder.display(),
        f = flaky.display(),
        m = marker.display(),
        out = record.display()
    ));
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "failed");
    std::fs::write(&marker, "").unwrap();

    let resumed = fixture
        .user
        .call(
            "workspace.prepare",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    let workspace = fixture.settled("login");

    assert_eq!(resumed["resumed"], json!({"redone": 0, "retried": 1}));
    assert_eq!(workspace["state"], "ready", "{workspace}");
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "first\nflaky\nflaky\nthird\n"
    );
    assert!(workspace["steps"][4]["failure"].is_null());
    // A ready workspace has nothing left to do.
    let again = fixture
        .user
        .call(
            "workspace.prepare",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    assert_eq!(again["resumed"], json!({"redone": 0, "retried": 0}));
    assert_eq!(fixture.settled("login")["state"], "ready");
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "first\nflaky\nflaky\nthird\n"
    );
}

#[test]
fn a_worktree_removed_by_hand_is_made_again_on_prepare() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    std::fs::remove_dir_all(fixture.workspace("login").join("lib")).unwrap();

    let resumed = fixture
        .user
        .call(
            "workspace.prepare",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();

    assert_eq!(resumed["resumed"], json!({"redone": 1, "retried": 0}));
    assert_eq!(fixture.settled("login")["state"], "ready");
    let lib = fixture.workspace("login").join("lib");
    // The branch an earlier attempt made is reused.
    assert_eq!(
        fixture.git(&lib, &["branch", "--show-current"]),
        "app/login"
    );
}

#[test]
fn the_variables_reach_a_run_step() {
    let fixture = Fixture::new(false);
    let out = fixture.user.path("env.txt");
    let script = fixture.script(
        "env.sh",
        "printf 'STORE=%s\\nCOMPOSE=%s\\nREPOSITORY=%s\\nPATH=%s\\n' \"$STORE\" \"$COMPOSE_PROJECT_NAME\" \"$REPOSITORY\" \"$PATH\" > \"$1\"\n",
    );
    fixture.configure(&format!(
        "[variables]\nSTORE = \"{{workspace.path}}/.store\"\nCOMPOSE_PROJECT_NAME = \"{{project}}-{{workspace}}\"\n\
         REPOSITORY = \"{{repository}}\"\nPATH = \"{{workspace.path}}/bin\"\n\
         [[setup]]\nrun = \"sh {} {}\"\nrepository = \"api\"\n",
        script.display(),
        out.display()
    ));

    fixture.create(json!({})).unwrap();
    let workspace = fixture.settled("login");

    assert_eq!(workspace["state"], "ready", "{workspace}");
    let folder = fixture.workspace("login");
    let text = std::fs::read_to_string(out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], format!("STORE={}/.store", folder.display()));
    assert_eq!(lines[1], "COMPOSE=app-login");
    assert_eq!(lines[2], "REPOSITORY=api");
    let path = lines[3].strip_prefix("PATH=").unwrap();
    assert!(
        path.starts_with(&format!("{}/bin:", folder.display())),
        "{path}"
    );
    assert!(
        path.len() > folder.display().to_string().len() + 5,
        "{path}"
    );
}

#[test]
fn removal_refuses_an_uncommitted_file_and_names_it() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    fixture.settled("login");
    std::fs::write(fixture.workspace("login").join("api/draft.txt"), "draft\n").unwrap();
    std::fs::write(
        fixture.workspace("login").join("web/README.md"),
        "changed\n",
    )
    .unwrap();

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains("web/README.md holds work that is not committed"),
        "{message}"
    );
    assert!(
        message.contains("api/draft.txt holds work that is not committed"),
        "{message}"
    );
    assert!(fixture.workspace("login").join("api/draft.txt").exists());
    assert!(fixture.show("login").is_ok());
}

#[test]
fn removal_refuses_an_unpushed_commit_and_names_it() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    fixture.settled("login");
    let lib = fixture.workspace("login").join("lib");
    std::fs::write(lib.join("feature.rs"), "fn f() {}\n").unwrap();
    fixture.user.git(&lib, &["add", "."]);
    fixture
        .user
        .git(&lib, &["commit", "--quiet", "-m", "add the feature"]);
    let commit = fixture.git(&lib, &["rev-parse", "--short", "HEAD"]);

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains(&format!(
            "lib has commits on no remote, on its branch app/login: {commit} add the feature"
        )),
        "{message}"
    );
    assert!(lib.join("feature.rs").exists());
}

#[test]
fn removal_succeeds_on_a_clean_pushed_workspace_and_leaves_git_worktree_list_clean() {
    let fixture = Fixture::new(true);
    std::fs::create_dir(fixture.folder.join("notes")).unwrap();
    std::fs::write(fixture.folder.join("web/.env"), "A=1\n").unwrap();
    fixture.configure(
        "[[setup]]\nroot_entries = true\n[[setup]]\ncopy = [\".env\"]\nfor_each_repository = true\n",
    );
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    let web = fixture.workspace("login").join("web");
    std::fs::write(web.join("feature.ts"), "export {}\n").unwrap();
    fixture.user.git(&web, &["add", "."]);
    fixture
        .user
        .git(&web, &["commit", "--quiet", "-m", "the feature"]);
    fixture
        .user
        .git(&web, &["push", "--quiet", "origin", "app/login"]);

    let removed = fixture.remove("login").unwrap();

    assert_eq!(removed["removed"], "login");
    assert_eq!(removed["project"], "app");
    assert!(!fixture.workspace("login").exists());
    for repository in [".", "web", "api", "lib"] {
        let main = fixture.folder.join(repository);
        let list = fixture.git(&main, &["worktree", "list", "--porcelain"]);
        assert_eq!(list.matches("worktree ").count(), 1, "{repository}: {list}");
    }
    assert!(fixture.folder.join("notes").is_dir());
    assert_eq!(fixture.show("login").unwrap_err().0, "not-found");
}

#[test]
fn removal_refuses_a_file_outside_every_repository() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    // Without the root repository the workspace folder is a plain folder no git status sees.
    let notes = fixture.workspace("login").join("notes.txt");
    std::fs::write(&notes, "kept\n").unwrap();

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(message.contains("notes.txt is in no worktree"), "{message}");
    assert!(notes.exists());
    assert!(fixture.workspace("login").join("api/.git").exists());
    assert!(fixture.show("login").is_ok());
}

#[test]
fn removal_refuses_a_repository_folder_whose_git_is_gone() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    let api = fixture.workspace("login").join("api");
    std::fs::remove_file(api.join(".git")).unwrap();
    std::fs::write(api.join("draft.txt"), "draft\n").unwrap();

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains("api/draft.txt is in no worktree"),
        "{message}"
    );
    assert!(api.join("draft.txt").exists());
    assert!(fixture.workspace("login").join("web/.git").exists());
    assert!(fixture.show("login").is_ok());
}

#[test]
fn removal_succeeds_when_the_root_repository_does_not_ignore_the_others() {
    let fixture = Fixture::new(true);
    // The root's `git status` then shows each nested worktree as an untracked folder.
    fixture
        .user
        .git(&fixture.folder, &["rm", "--quiet", ".gitignore"]);
    fixture.user.git(
        &fixture.folder,
        &["commit", "--quiet", "-m", "ignore nothing"],
    );
    fixture
        .user
        .git(&fixture.folder, &["push", "--quiet", "origin", "main"]);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");

    let removed = fixture.remove("login");

    assert_eq!(removed.unwrap()["removed"], "login");
    assert!(!fixture.workspace("login").exists());
}

#[test]
fn the_command_line_creates_lists_shows_prepares_and_removes_a_workspace() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    let run = |args: &[&str]| {
        let output = fixture.user.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    let created: Value = serde_json::from_str(&run(&[
        "workspace",
        "create",
        "app",
        "login",
        "--repositories",
        "web,lib",
        "--branch",
        "feature/login",
        "--json",
    ]))
    .unwrap();
    fixture.settled("login");
    let listed: Value =
        serde_json::from_str(&run(&["workspace", "list", "app", "--json"])).unwrap();
    let shown = run(&["workspace", "show", "app", "login"]);
    let prepared = run(&["workspace", "prepare", "app", "login"]);
    let removed = run(&["workspace", "remove", "app", "login"]);

    assert_eq!(created["branch"], "feature/login");
    let paths: Vec<_> = created["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["web", "lib"]);
    assert_eq!(listed["workspaces"][0]["name"], "login");
    assert!(
        shown.contains("login, branch feature/login, ready"),
        "{shown}"
    );
    assert!(shown.contains("web: develop"), "{shown}");
    assert!(prepared.contains("Nothing to resume"), "{prepared}");
    assert!(removed.contains("Removed workspace login"), "{removed}");
}

// The workspace's local branches on removal: deleted once their work has landed, never the remote
// one, asked about when work would be lost.

/// The fixture with the fake `gh`, its workspace `login` made and ready.
fn landing() -> Fixture {
    let fixture = Fixture::of(User::new().with_fake_gh(), false);
    fixture.configure("");
    fixture.create(json!({})).unwrap();
    assert_eq!(fixture.settled("login")["state"], "ready");
    fixture
}

impl Fixture {
    /// A commit in `repository` of the workspace `login`, pushed to its remote when `pushed`.
    fn commit_in(&self, repository: &str, pushed: bool) -> String {
        let dir = self.workspace("login").join(repository);
        std::fs::write(dir.join("feature.rs"), "fn f() {}\n").unwrap();
        self.user.git(&dir, &["add", "."]);
        self.user
            .git(&dir, &["commit", "--quiet", "-m", "add the feature"]);
        if pushed {
            self.user
                .git(&dir, &["push", "--quiet", "origin", "app/login"]);
        }
        self.git(&dir, &["rev-parse", "--short", "HEAD"])
    }

    /// `lib` on GitHub as `acme/lib`: its remote's URL leads there (its tracking refs stay what
    /// they were), and the project declares it.
    fn lib_on_github(&self) {
        let url = "git@github.com:acme/lib.git";
        self.user.git(
            &self.folder.join("lib"),
            &["remote", "set-url", "origin", url],
        );
        self.configure("");
        let text = std::fs::read_to_string(self.user.project_file("app"))
            .unwrap()
            .replace(
                "path = \"lib\"\n",
                "path = \"lib\"\nforge = { kind = \"github\", repository = \"acme/lib\" }\n",
            );
        self.user.write_project_file("app", &text);
    }

    /// The pull request of `app/login` on `acme/lib`, in that state.
    fn lib_pull(&self, state: &str) {
        self.user
            .gh_script(json!({"pulls": {"acme/lib": {"app/login": {
                "number": 12, "title": "Add the feature", "state": state,
                "baseRefName": "main", "url": "https://github.com/acme/lib/pull/12"
            }}}}));
    }

    /// The pull request of `app/login` on `acme/lib`, in that state, its head at `head`.
    fn lib_pull_at(&self, state: &str, head: &str) {
        self.user
            .gh_script(json!({"pulls": {"acme/lib": {"app/login": {
                "number": 12, "title": "Add the feature", "state": state, "headRefOid": head,
                "baseRefName": "main", "url": "https://github.com/acme/lib/pull/12"
            }}}}));
    }

    /// `lib`'s commit pushed, then its remote branch deleted, as the forge does on a merge, and
    /// pruned from the main checkout by a fetch: its full hash.
    fn lib_pushed_then_pruned(&self) -> String {
        self.commit_in("lib", true);
        let head = self.git(&self.workspace("login").join("lib"), &["rev-parse", "HEAD"]);
        self.user.git(
            &self.remote("lib"),
            &["branch", "--quiet", "-D", "app/login"],
        );
        self.user.git(
            &self.folder.join("lib"),
            &["fetch", "--quiet", "--prune", "origin"],
        );
        head
    }

    fn has_branch(&self, repository: &Path) -> bool {
        self.git(repository, &["branch", "--list", "app/login"])
            .contains("app/login")
    }

    /// What the removal said of the branch in `repository`: `(deleted, why)`.
    fn fate(removed: &Value, repository: &str) -> (bool, String) {
        let fate = removed["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["repository"] == repository)
            .unwrap_or_else(|| panic!("no branch of {repository}: {removed}"));
        (
            fate["deleted"].as_bool().unwrap(),
            fate["why"].as_str().unwrap().to_owned(),
        )
    }
}

#[test]
fn a_branch_with_no_commit_of_its_own_is_deleted() {
    let fixture = landing();

    let removed = fixture.remove("login").unwrap();

    for repository in ["web", "api", "lib"] {
        assert!(
            !fixture.has_branch(&fixture.folder.join(repository)),
            "{repository}"
        );
        let (deleted, why) = Fixture::fate(&removed, repository);
        assert!(deleted, "{repository}: {why}");
        assert_eq!(why, "it has no commit of its own");
    }
    // The forge is not asked about a branch whose work is all in its base.
    assert!(fixture.user.gh_received().is_empty());
}

#[test]
fn a_branch_whose_pull_request_was_squash_merged_is_deleted_locally() {
    let fixture = landing();
    fixture.commit_in("lib", true);
    fixture.lib_on_github();
    fixture.lib_pull("MERGED");

    let removed = fixture.remove("login").unwrap();

    assert!(!fixture.has_branch(&fixture.folder.join("lib")));
    assert_eq!(
        Fixture::fate(&removed, "lib"),
        (true, "its pull request #12 is merged".to_owned())
    );
}

#[test]
fn the_remote_branch_stays() {
    let fixture = landing();
    fixture.commit_in("lib", true);
    fixture.lib_on_github();
    fixture.lib_pull("MERGED");

    fixture.remove("login").unwrap();

    assert!(!fixture.has_branch(&fixture.folder.join("lib")));
    assert!(fixture.has_branch(&fixture.remote("lib")));
    assert!(
        fixture
            .git(&fixture.folder.join("lib"), &["branch", "--remotes"])
            .contains("origin/app/login")
    );
}

#[test]
fn a_branch_with_unpushed_commits_makes_the_removal_ask_with_its_reason() {
    let fixture = landing();
    let commit = fixture.commit_in("lib", false);

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains(&format!(
            "lib has commits on no remote, on its branch app/login: {commit} add the feature"
        )),
        "{message}"
    );
    assert!(fixture.workspace("login").join("lib/feature.rs").exists());
    assert!(fixture.has_branch(&fixture.folder.join("lib")));
}

#[test]
fn a_branch_whose_pull_request_is_open_is_kept_and_says_why() {
    let fixture = landing();
    fixture.commit_in("lib", true);
    fixture.lib_on_github();
    fixture.lib_pull("OPEN");

    let removed = fixture.remove("login").unwrap();

    assert!(fixture.has_branch(&fixture.folder.join("lib")));
    assert_eq!(
        Fixture::fate(&removed, "lib"),
        (false, "its pull request #12 is open, not merged".to_owned())
    );
}

#[test]
fn a_branch_the_forge_cannot_say_merged_is_kept() {
    let fixture = landing();
    fixture.commit_in("lib", true);
    fixture.commit_in("web", true);
    fixture.lib_on_github();
    fixture
        .user
        .gh_script(json!({"fail": {"acme/lib": "HTTP 502: Bad Gateway"}}));

    let removed = fixture.remove("login").unwrap();

    assert!(fixture.has_branch(&fixture.folder.join("lib")));
    let (deleted, why) = Fixture::fate(&removed, "lib");
    assert!(!deleted);
    assert!(
        why.starts_with("the forge cannot say whether it is merged: ") && why.contains("502"),
        "{why}"
    );
    // No forge declared: nothing says it is merged.
    assert!(fixture.has_branch(&fixture.folder.join("web")));
    let (deleted, why) = Fixture::fate(&removed, "web");
    assert!(!deleted);
    assert!(
        why.starts_with("no forge says whether it is merged: "),
        "{why}"
    );
}

#[test]
fn the_command_line_says_which_branches_it_deleted_and_which_it_kept() {
    let fixture = landing();
    fixture.commit_in("lib", true);
    fixture.lib_on_github();
    fixture.lib_pull("OPEN");

    let output = fixture.user.run(&["workspace", "remove", "app", "login"]);

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{text}");
    assert_eq!(
        text,
        "Removed workspace login of app: its worktrees and its folder. Its branch app/login was \
         deleted in web, api: it has no commit of its own. It is kept in lib: its pull request \
         #12 is open, not merged.\n"
    );
}

#[test]
fn a_squash_merged_branch_whose_remote_was_pruned_is_removed_and_deleted_locally() {
    let fixture = landing();
    let head = fixture.lib_pushed_then_pruned();
    fixture.lib_on_github();
    fixture.lib_pull_at("MERGED", &head);

    let removed = fixture.remove("login").unwrap();

    assert!(!fixture.workspace("login").exists());
    assert!(!fixture.has_branch(&fixture.folder.join("lib")));
    assert_eq!(
        Fixture::fate(&removed, "lib"),
        (true, "its pull request #12 is merged".to_owned())
    );
}

#[test]
fn a_pruned_branch_whose_pull_request_is_not_merged_still_makes_the_removal_ask() {
    let fixture = landing();
    let head = fixture.lib_pushed_then_pruned();
    fixture.lib_on_github();
    fixture.lib_pull_at("OPEN", &head);

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains("lib has commits on no remote, on its branch app/login: "),
        "{message}"
    );
    assert!(fixture.has_branch(&fixture.folder.join("lib")));
}

#[test]
fn a_pruned_branch_the_forge_cannot_say_merged_still_makes_the_removal_ask() {
    let fixture = landing();
    fixture.lib_pushed_then_pruned();
    fixture.lib_on_github();
    fixture
        .user
        .gh_script(json!({"fail": {"acme/lib": "HTTP 502: Bad Gateway"}}));

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains("lib has commits on no remote"),
        "{message}"
    );
    assert!(fixture.has_branch(&fixture.folder.join("lib")));
}

#[test]
fn a_commit_made_after_the_merge_of_a_pruned_branch_still_makes_the_removal_ask() {
    let fixture = landing();
    let head = fixture.lib_pushed_then_pruned();
    let lib = fixture.workspace("login").join("lib");
    std::fs::write(lib.join("later.rs"), "fn later() {}\n").unwrap();
    fixture.user.git(&lib, &["add", "."]);
    fixture
        .user
        .git(&lib, &["commit", "--quiet", "-m", "after the merge"]);
    let later = fixture.git(&lib, &["rev-parse", "--short", "HEAD"]);
    fixture.lib_on_github();
    fixture.lib_pull_at("MERGED", &head);

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains(&format!(
            "lib has commits on no remote, on its branch app/login: {later} after the merge"
        )),
        "{message}"
    );
    assert!(fixture.has_branch(&fixture.folder.join("lib")));
}

#[test]
fn an_uncommitted_file_still_refuses_the_removal_of_a_merged_pruned_branch() {
    let fixture = landing();
    let head = fixture.lib_pushed_then_pruned();
    fixture.lib_on_github();
    fixture.lib_pull_at("MERGED", &head);
    std::fs::write(fixture.workspace("login").join("lib/draft.txt"), "draft\n").unwrap();

    let (code, message) = fixture.remove("login").unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains("lib/draft.txt holds work that is not committed"),
        "{message}"
    );
    assert!(!message.contains("on no remote"), "{message}");
    assert!(fixture.has_branch(&fixture.folder.join("lib")));
}

// The same on Bitbucket, whose pull request says its source commit through `bkt` (Cloud:
// `source.commit.hash`, Data Center: `fromRef.latestCommit`).

/// Where a Bitbucket checkout of `lib` lives.
#[derive(Clone, Copy)]
enum Host {
    Cloud,
    DataCenter,
}

const HOSTS: [Host; 2] = [Host::Cloud, Host::DataCenter];

impl Host {
    fn name(self) -> &'static str {
        match self {
            Host::Cloud => "Cloud",
            Host::DataCenter => "Data Center",
        }
    }
}

impl Fixture {
    /// The fixture with the fake `bkt`, its workspace `login` made and ready.
    fn landing_on_bitbucket() -> Fixture {
        let fixture = Fixture::of(User::new().with_fake_bkt(), false);
        fixture.configure("");
        fixture.create(json!({})).unwrap();
        assert_eq!(fixture.settled("login")["state"], "ready");
        fixture
    }

    /// `lib` on Bitbucket, as `acme/lib` (Cloud) or `PROJ/lib` (Data Center): its remote's URL
    /// leads there (its tracking refs stay what they were), and the project declares it.
    fn lib_on_bitbucket(&self, host: Host) {
        let (url, forge) = match host {
            Host::Cloud => (
                "git@bitbucket.org:acme/lib.git",
                "{ kind = \"bitbucket-cloud\", repository = \"acme/lib\" }",
            ),
            Host::DataCenter => (
                "ssh://git@bitbucket.example.com:7999/PROJ/lib.git",
                "{ kind = \"bitbucket-data-center\", host = \"bitbucket.example.com\", \
                 repository = \"PROJ/lib\" }",
            ),
        };
        self.user.git(
            &self.folder.join("lib"),
            &["remote", "set-url", "origin", url],
        );
        self.configure("");
        let text = std::fs::read_to_string(self.user.project_file("app"))
            .unwrap()
            .replace(
                "path = \"lib\"\n",
                &format!("path = \"lib\"\nforge = {forge}\n"),
            );
        self.user.write_project_file("app", &text);
    }

    /// The pull request #12 of `app/login` on Bitbucket, in that state, its source commit at
    /// `head` when there is one, as `bkt pr list --json` prints it.
    fn lib_bitbucket_pull(&self, host: Host, state: &str, head: Option<&str>) {
        let (pull, api) = match host {
            Host::Cloud => {
                let mut source = json!({
                    "branch": {"name": "app/login"}, "repository": {"full_name": "acme/lib"}});
                if let Some(head) = head {
                    source["commit"] = json!({"hash": head});
                }
                let base = "/repositories/acme/lib/pullrequests/12";
                (
                    json!({"id": 12, "title": "Add the feature", "state": state, "source": source,
                        "destination": {"branch": {"name": "main"},
                            "repository": {"full_name": "acme/lib"}},
                        "links": {"html": {"href":
                            "https://bitbucket.org/acme/lib/pull-requests/12"}}}),
                    json!({base: {"participants": []},
                        format!("{base}/comments"): {"values": []}}),
                )
            }
            Host::DataCenter => {
                let repository = json!({"slug": "lib", "project": {"key": "PROJ"}});
                let mut from = json!({"displayId": "app/login", "repository": repository});
                if let Some(head) = head {
                    from["latestCommit"] = json!(head);
                }
                (
                    json!({"id": 12, "title": "Add the feature", "state": state, "fromRef": from,
                        "toRef": {"displayId": "main", "repository": repository},
                        "links": {"self": [{"href":
                            "https://bitbucket.example.com/projects/PROJ/repos/lib/pull-requests/12"}]}}),
                    json!({"/rest/api/1.0/projects/PROJ/repos/lib/pull-requests/12/activities":
                        {"values": []}}),
                )
            }
        };
        let context = match host {
            Host::Cloud => json!({"name": "cloud", "host": "bitbucket.org"}),
            Host::DataCenter => json!({"name": "work", "host": "bitbucket.example.com:7990"}),
        };
        let key = match host {
            Host::Cloud => "acme/lib",
            Host::DataCenter => "PROJ/lib",
        };
        self.user
            .bkt_script(json!({"contexts": [context], "pulls": {key: [pull]}, "api": api}));
    }
}

#[test]
fn a_squash_merged_cloud_branch_whose_remote_was_pruned_is_removed_and_deleted_locally() {
    let fixture = Fixture::landing_on_bitbucket();
    let head = fixture.lib_pushed_then_pruned();
    fixture.lib_on_bitbucket(Host::Cloud);
    fixture.lib_bitbucket_pull(Host::Cloud, "MERGED", Some(&head));

    let removed = fixture.remove("login").unwrap();

    assert!(!fixture.workspace("login").exists());
    assert!(!fixture.has_branch(&fixture.folder.join("lib")));
    assert_eq!(
        Fixture::fate(&removed, "lib"),
        (true, "its pull request #12 is merged".to_owned())
    );
}

#[test]
fn a_squash_merged_data_center_branch_whose_remote_was_pruned_is_removed_and_deleted_locally() {
    let fixture = Fixture::landing_on_bitbucket();
    let head = fixture.lib_pushed_then_pruned();
    fixture.lib_on_bitbucket(Host::DataCenter);
    fixture.lib_bitbucket_pull(Host::DataCenter, "MERGED", Some(&head));

    let removed = fixture.remove("login").unwrap();

    assert!(!fixture.workspace("login").exists());
    assert!(!fixture.has_branch(&fixture.folder.join("lib")));
    assert_eq!(
        Fixture::fate(&removed, "lib"),
        (true, "its pull request #12 is merged".to_owned())
    );
}

#[test]
fn a_bitbucket_hash_cut_short_is_still_the_commit_the_pull_request_merged() {
    // Cloud may print a source commit's hash cut short; git resolves the prefix.
    let fixture = Fixture::landing_on_bitbucket();
    let head = fixture.lib_pushed_then_pruned();
    fixture.lib_on_bitbucket(Host::Cloud);
    fixture.lib_bitbucket_pull(Host::Cloud, "MERGED", Some(&head[..12]));

    fixture.remove("login").unwrap();

    assert!(!fixture.workspace("login").exists());
    assert!(!fixture.has_branch(&fixture.folder.join("lib")));
}

#[test]
fn a_pruned_bitbucket_branch_whose_pull_request_is_not_merged_still_makes_the_removal_ask() {
    for host in HOSTS {
        let fixture = Fixture::landing_on_bitbucket();
        let head = fixture.lib_pushed_then_pruned();
        fixture.lib_on_bitbucket(host);
        fixture.lib_bitbucket_pull(host, "OPEN", Some(&head));

        let (code, message) = fixture.remove("login").unwrap_err();

        assert_eq!(code, "refused", "{}", host.name());
        assert!(
            message.contains("lib has commits on no remote, on its branch app/login: "),
            "{}: {message}",
            host.name()
        );
        assert!(fixture.has_branch(&fixture.folder.join("lib")));
    }
}

#[test]
fn a_merged_bitbucket_pull_request_that_says_no_source_commit_still_makes_the_removal_ask() {
    for host in HOSTS {
        let fixture = Fixture::landing_on_bitbucket();
        fixture.lib_pushed_then_pruned();
        fixture.lib_on_bitbucket(host);
        fixture.lib_bitbucket_pull(host, "MERGED", None);

        let (code, message) = fixture.remove("login").unwrap_err();

        assert_eq!(code, "refused", "{}", host.name());
        assert!(
            message.contains("lib has commits on no remote"),
            "{}: {message}",
            host.name()
        );
        assert!(fixture.has_branch(&fixture.folder.join("lib")));
    }
}

#[test]
fn a_source_commit_that_is_not_a_hash_is_never_given_to_git() {
    for host in HOSTS {
        let fixture = Fixture::landing_on_bitbucket();
        fixture.lib_pushed_then_pruned();
        fixture.lib_on_bitbucket(host);
        for head in ["--output=/tmp/x", "abc12", "HEAD", "main"] {
            fixture.lib_bitbucket_pull(host, "MERGED", Some(head));

            let (code, message) = fixture.remove("login").unwrap_err();

            assert_eq!(code, "refused", "{} {head}", host.name());
            assert!(
                message.contains("lib has commits on no remote"),
                "{} {head}: {message}",
                host.name()
            );
            assert!(fixture.has_branch(&fixture.folder.join("lib")));
        }
    }
}

#[test]
fn a_commit_made_after_the_merge_of_a_pruned_bitbucket_branch_still_makes_the_removal_ask() {
    for host in HOSTS {
        let fixture = Fixture::landing_on_bitbucket();
        let head = fixture.lib_pushed_then_pruned();
        let lib = fixture.workspace("login").join("lib");
        std::fs::write(lib.join("later.rs"), "fn later() {}\n").unwrap();
        fixture.user.git(&lib, &["add", "."]);
        fixture
            .user
            .git(&lib, &["commit", "--quiet", "-m", "after the merge"]);
        let later = fixture.git(&lib, &["rev-parse", "--short", "HEAD"]);
        fixture.lib_on_bitbucket(host);
        fixture.lib_bitbucket_pull(host, "MERGED", Some(&head));

        let (code, message) = fixture.remove("login").unwrap_err();

        assert_eq!(code, "refused", "{}", host.name());
        assert!(
            message.contains(&format!(
                "lib has commits on no remote, on its branch app/login: {later} after the merge"
            )),
            "{}: {message}",
            host.name()
        );
        assert!(fixture.has_branch(&fixture.folder.join("lib")));
    }
}

// The output of a setup step (`workspace.step_output`).

impl Fixture {
    fn step_output(&self, position: u64) -> Result<Value, (String, String)> {
        self.user.call(
            "workspace.step_output",
            json!({"project": "app", "name": "login", "position": position}),
        )
    }

    /// Waits for a file a script made.
    fn appears(&self, path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "{} never appeared",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn a_running_setup_step_streams_its_output() {
    let fixture = Fixture::new(false);
    let script = fixture.script(
        "stream.sh",
        "echo first line\ntouch \"$1\"\nwhile [ ! -f \"$2\" ]; do sleep 0.05; done\necho last line\n",
    );
    let (written, release) = (fixture.user.path("written"), fixture.user.path("release"));
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {} {} {}\"\nrepository = \"lib\"\n",
        script.display(),
        written.display(),
        release.display()
    ));

    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    fixture.appears(&written);
    let deadline = Instant::now() + Duration::from_secs(10);
    let during = loop {
        let during = fixture.step_output(1).unwrap();
        if during["output"] == "first line" {
            break during;
        }
        assert!(Instant::now() < deadline, "{during}");
        std::thread::sleep(Duration::from_millis(50));
    };

    assert_eq!(during["state"], "running", "{during}");
    assert_eq!(during["earlier"], false);
    assert!(during["started_at"].is_u64() && during["ended_at"].is_null());
    std::fs::write(&release, "").unwrap();
    let workspace = fixture.settled("login");
    assert_eq!(workspace["state"], "ready", "{workspace}");
    assert_eq!(
        fixture.step_output(1).unwrap()["output"],
        "first line\nlast line"
    );
}

#[test]
fn a_finished_setup_step_shows_its_output() {
    let fixture = Fixture::new(false);
    let script = fixture.script("done.sh", "echo one\necho two\n");
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {}\"\nrepository = \"lib\"\n",
        script.display()
    ));

    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    fixture.settled("login");
    let read = fixture.step_output(1).unwrap();

    assert_eq!(read["state"], "done", "{read}");
    assert_eq!(read["output"], "one\ntwo");
    assert_eq!(read["line"], format!("sh {} (in lib)", script.display()));
    assert_eq!(
        read["folder"],
        fixture.workspace("login").join("lib").display().to_string()
    );
    assert!(read["ended_at"].is_u64(), "{read}");
    // The worktree step before it wrote nothing.
    assert_eq!(fixture.step_output(0).unwrap()["output"], "");
}

#[test]
fn a_failed_setup_step_shows_its_whole_output_not_only_the_tail() {
    let fixture = Fixture::new(false);
    let failing = fixture.script(
        "fail.sh",
        "i=1\nwhile [ $i -le 50 ]; do echo \"line $i\"; i=$((i+1)); done\nexit 3\n",
    );
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {}\"\nrepository = \"lib\"\n",
        failing.display()
    ));

    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    let workspace = fixture.settled("login");
    let read = fixture.step_output(1).unwrap();

    assert_eq!(read["state"], "failed", "{read}");
    let whole: Vec<String> = (1..=50).map(|i| format!("line {i}")).collect();
    assert_eq!(read["output"], whole.join("\n"));
    // The card of the workspace still keeps its short tail.
    let kept = workspace["steps"][1]["failure"]["output"].as_str().unwrap();
    assert_eq!(kept.lines().count(), 40, "{kept}");
}

#[test]
fn a_setup_step_that_fails_without_a_program_shows_why() {
    let fixture = Fixture::new(false);
    fixture.configure("[[setup]]\nlink = \"missing.txt\"\nrepository = \"lib\"\n");

    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    fixture.settled("login");
    let read = fixture.step_output(1).unwrap();

    assert_eq!(read["state"], "failed", "{read}");
    assert!(
        read["output"].as_str().unwrap().contains("missing.txt"),
        "{read}"
    );
}

#[test]
fn a_secret_in_a_setup_step_s_output_is_masked_while_it_runs_and_once_it_ended() {
    let fixture = Fixture::new(false);
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let script = fixture.script(
        "secret.sh",
        &format!(
            "echo \"token {secret}\"\ntouch \"$1\"\nwhile [ ! -f \"$2\" ]; do sleep 0.05; done\n"
        ),
    );
    let (written, release) = (fixture.user.path("written"), fixture.user.path("release"));
    fixture.configure(&format!(
        "[[setup]]\nrun = \"sh {} {} {}\"\nrepository = \"lib\"\n",
        script.display(),
        written.display(),
        release.display()
    ));

    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    fixture.appears(&written);
    let deadline = Instant::now() + Duration::from_secs(10);
    let during = loop {
        let during = fixture.step_output(1).unwrap();
        if during["output"] != "" {
            break during;
        }
        assert!(Instant::now() < deadline, "{during}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(during["output"], "token [masked]", "{during}");
    std::fs::write(&release, "").unwrap();
    fixture.settled("login");

    let after = fixture.step_output(1).unwrap();
    assert_eq!(after["output"], "token [masked]", "{after}");
    // Nothing of the store's files (the database and its journal) holds it either.
    let folder = fixture.user.path("state/flottille");
    let mut seen = 0;
    for file in std::fs::read_dir(&folder).unwrap() {
        let bytes = std::fs::read(file.unwrap().path()).unwrap_or_default();
        seen += bytes.len();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(secret),
            "the secret reached the store"
        );
    }
    assert!(seen > 0, "no store file was read in {}", folder.display());
}

#[test]
fn a_setup_step_that_does_not_exist_is_not_found() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    fixture.create(json!({"repositories": ["lib"]})).unwrap();
    fixture.settled("login");

    let missing = fixture.step_output(9).unwrap_err();
    assert_eq!(missing.0, "not-found", "{missing:?}");
    let ghost = fixture.user.call(
        "workspace.step_output",
        json!({"project": "app", "name": "ghost", "position": 0}),
    );
    assert_eq!(ghost.unwrap_err().0, "not-found");
}

// The base branches and the dependencies of a repository, changed from the project settings
// (#172).

fn base_of(workspace: &Value, path: &str) -> Value {
    workspace["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"] == path)
        .unwrap_or_else(|| panic!("{path} in {workspace}"))["base"]["branch"]
        .clone()
}

fn set_repository(fixture: &Fixture, setting: Value) -> Result<Value, (String, String)> {
    fixture.user.call(
        "project.set_settings",
        json!({"name": "app", "repositories": [setting]}),
    )
}

#[test]
fn base_branches_changed_from_the_settings_are_used_by_the_next_workspace() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    let before = fixture.create(json!({"name": "first"})).unwrap();
    assert_eq!(base_of(&before, "web"), "develop", "the file's rule");

    let saved = set_repository(
        &fixture,
        json!({"path": "web", "base": ["main", "develop"]}),
    )
    .unwrap();

    let web = saved["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"] == "web")
        .unwrap();
    assert_eq!(web["base"], json!(["main", "develop"]), "{saved}");
    let text = std::fs::read_to_string(fixture.user.project_file("app")).unwrap();
    assert!(text.contains("base = [\"main\", \"develop\"]"), "{text}");
    let after = fixture.create(json!({"name": "second"})).unwrap();
    assert_eq!(base_of(&after, "web"), "main", "the order is the rule");
    assert_eq!(base_of(&after, "api"), "develop", "the others keep theirs");
    assert_eq!(
        base_of(&before, "web"),
        "develop",
        "a workspace already made keeps its base"
    );
}

#[test]
fn a_repository_left_out_of_a_save_keeps_its_bases_and_dependencies() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    set_repository(
        &fixture,
        json!({"path": "web", "base": ["main"], "depends_on": ["api"]}),
    )
    .unwrap();

    // A save that says nothing of them (a script, an older client) leaves them as they are.
    let saved = set_repository(&fixture, json!({"path": "web", "forge": null})).unwrap();

    let web = &saved["repositories"][0];
    assert_eq!(web["path"], "web");
    assert_eq!(web["depends_on"], json!(["api"]), "{saved}");
    let shown = fixture
        .user
        .call("project.repositories", json!({"name": "app"}))
        .unwrap();
    assert_eq!(shown["repositories"][0]["base"], json!(["main"]), "{shown}");
}

#[test]
fn a_base_branch_missing_on_the_remote_is_refused_with_its_reason() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    let before = std::fs::read_to_string(fixture.user.project_file("app")).unwrap();

    // `lib` has no `develop` on its remote, and `trunk` is on none.
    for (path, base, why) in [
        (
            "lib",
            json!(["main", "develop"]),
            "lib: develop is not a branch of the remote origin; nothing was saved",
        ),
        (
            "web",
            json!(["trunk"]),
            "web: trunk is not a branch of the remote origin; nothing was saved",
        ),
    ] {
        let (code, message) =
            set_repository(&fixture, json!({"path": path, "base": base})).unwrap_err();
        assert_eq!(code, "refused");
        assert_eq!(message, why);
    }
    assert_eq!(
        std::fs::read_to_string(fixture.user.project_file("app")).unwrap(),
        before,
        "nothing was written"
    );
}

#[test]
fn a_base_rule_with_no_branch_or_a_branch_with_a_space_is_refused() {
    let fixture = Fixture::new(false);
    fixture.configure("");

    for base in [json!([]), json!(["main develop"]), json!([""])] {
        let (code, message) =
            set_repository(&fixture, json!({"path": "web", "base": base})).unwrap_err();
        assert_eq!(code, "refused", "{message}");
        assert!(message.contains("nothing was saved"), "{message}");
    }
}

#[test]
fn a_remote_that_cannot_be_read_is_refused_unless_the_branch_was_fetched() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    let web = fixture.folder.join("web");
    fixture.git(&web, &["fetch", "--quiet", "origin"]);
    let moved = fixture.user.path("remotes/gone.git");
    std::fs::rename(fixture.remote("web"), &moved).unwrap();

    set_repository(&fixture, json!({"path": "web", "base": ["develop"]}))
        .expect("develop was fetched: its tracking ref says it is there");
    let (code, message) =
        set_repository(&fixture, json!({"path": "web", "base": ["trunk"]})).unwrap_err();
    assert_eq!(code, "refused");
    assert!(
        message.starts_with("web: the remote origin could not be read ("),
        "{message}"
    );
    assert!(
        message.ends_with("and trunk was never fetched from it; nothing was saved"),
        "{message}"
    );
}

#[test]
fn a_repository_with_no_remote_takes_its_own_branches_as_bases() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    let lib = fixture.folder.join("lib");
    fixture.git(&lib, &["remote", "remove", "origin"]);

    set_repository(&fixture, json!({"path": "lib", "base": ["main"]})).unwrap();
    let (code, message) =
        set_repository(&fixture, json!({"path": "lib", "base": ["develop"]})).unwrap_err();

    assert_eq!(code, "refused");
    assert_eq!(
        message,
        "lib: develop is not a branch of lib, which has no remote origin; nothing was saved"
    );
}

#[test]
fn dependencies_changed_from_the_settings_are_saved_and_shown() {
    let fixture = Fixture::new(false);
    fixture.configure("");

    let saved = set_repository(
        &fixture,
        json!({"path": "web", "depends_on": ["api", "lib"]}),
    )
    .unwrap();

    assert_eq!(
        saved["repositories"][0]["depends_on"],
        json!(["api", "lib"])
    );
    let text = std::fs::read_to_string(fixture.user.project_file("app")).unwrap();
    assert!(text.contains("depends_on = [\"api\", \"lib\"]"), "{text}");

    let cleared = set_repository(&fixture, json!({"path": "web", "depends_on": []})).unwrap();
    assert_eq!(cleared["repositories"][0]["depends_on"], json!([]));
    let text = std::fs::read_to_string(fixture.user.project_file("app")).unwrap();
    assert!(!text.contains("depends_on"), "{text}");
}

#[test]
fn a_dependency_cycle_is_refused() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    set_repository(&fixture, json!({"path": "web", "depends_on": ["api"]})).unwrap();
    set_repository(&fixture, json!({"path": "api", "depends_on": ["lib"]})).unwrap();
    let before = std::fs::read_to_string(fixture.user.project_file("app")).unwrap();

    let (code, message) =
        set_repository(&fixture, json!({"path": "lib", "depends_on": ["web"]})).unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.contains("depends_on: cycle web -> api -> lib -> web")
            && message.contains("nothing was saved"),
        "{message}"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.user.project_file("app")).unwrap(),
        before
    );
}

#[test]
fn a_dependency_on_itself_or_on_a_repository_the_project_lacks_is_refused() {
    let fixture = Fixture::new(false);
    fixture.configure("");

    let (code, message) =
        set_repository(&fixture, json!({"path": "web", "depends_on": ["web"]})).unwrap_err();
    assert_eq!(code, "refused");
    assert!(
        message.contains("a repository cannot depend on itself")
            && message.contains("nothing was saved"),
        "{message}"
    );
    let (code, message) =
        set_repository(&fixture, json!({"path": "web", "depends_on": ["docs"]})).unwrap_err();
    assert_eq!(code, "refused");
    assert!(message.contains("unknown repository \"docs\""), "{message}");
}
