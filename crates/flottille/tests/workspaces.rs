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
        let user = User::new();
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
    /// then `rest` (variables, recipe…).
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
        "[[recipe]]\nrun = \"sh {}\"\nrepository = \"lib\"\n",
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

#[test]
fn the_fetch_failing_stops_the_creation_with_the_reason() {
    let fixture = Fixture::new(false);
    fixture.configure("");
    std::fs::rename(fixture.remote("lib"), fixture.user.path("remotes/gone.git")).unwrap();

    let (code, message) = fixture.create(json!({})).unwrap_err();

    assert_eq!(code, "refused");
    assert!(
        message.starts_with("cannot fetch the base of lib from origin: "),
        "{message}"
    );
    assert!(message.contains("--offline"), "{message}");
    assert!(!fixture.workspace("login").exists());
    assert_eq!(fixture.show("login").unwrap_err().0, "not-found");
    assert!(
        fixture
            .git(
                &fixture.folder.join("web"),
                &["branch", "--list", "app/login"]
            )
            .is_empty()
    );
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
        "[[recipe]]\ncopy = [\".env\", \".env.*\", \"config.json\", \".env.missing\"]\nrepository = \"api\"\n",
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
    fixture.configure("[[recipe]]\nroot_entries = true\n");

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
        "[[recipe]]\nrun = \"sh {} {{repository}} {}\"\nfor_each_repository = true\n",
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
        "[[recipe]]\nrun = \"sh {}\"\nrepository = \"api\"\n[[recipe]]\nrun = \"sh {} {}\"\n",
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
        "[[recipe]]\nrun = \"sh {} {}\"\nrepository = \"lib\"\n",
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
        "[[recipe]]\nrun = \"sh {}\"\nrepository = \"lib\"\ntime_limit = 1\n",
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
        "[[recipe]]\nrun = \"sh {r} first {out}\"\n\
         [[recipe]]\nrun = \"sh {f} {m} {out}\"\n\
         [[recipe]]\nrun = \"sh {r} third {out}\"\n",
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
         [[recipe]]\nrun = \"sh {} {}\"\nrepository = \"api\"\n",
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
            "lib has commits on no remote: {commit} add the feature"
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
        "[[recipe]]\nroot_entries = true\n[[recipe]]\ncopy = [\".env\"]\nfor_each_repository = true\n",
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

    assert_eq!(removed, json!({"removed": "login", "project": "app"}));
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

    assert_eq!(removed, Ok(json!({"removed": "login", "project": "app"})));
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
