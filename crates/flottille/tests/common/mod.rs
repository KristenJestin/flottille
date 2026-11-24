//! What the integration tests share: a throwaway user and the daemon it starts.

// Each test file uses its own part of it.
#![allow(dead_code)]

pub mod mcp;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

/// None of the machine's git settings, and a throwaway identity.
const GIT_ENV: [(&str, &str); 6] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "Test"),
    ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
    ("GIT_COMMITTER_NAME", "Test"),
    ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
];

/// What a test names that would reach the machine outside the test's folders if it ran: faked by
/// `User::with_fake_launchers`.
const LAUNCHERS: [&str; 40] = [
    "systemd-run",
    "systemctl",
    "loginctl",
    "tmux",
    "screen",
    "zellij",
    "dtach",
    "abduco",
    "setsid",
    "at",
    "batch",
    "crontab",
    "dbus-send",
    "gdbus",
    "busctl",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "docker",
    "podman",
    "nerdctl",
    "hyprctl",
    "swaymsg",
    "i3-msg",
    "niri",
    "kitty",
    "wezterm",
    "aws",
    "kubectl",
    "gcloud",
    "secret-tool",
    "pass",
    "op",
    "bw",
    "uv",
    "ionice",
    "xdg-open",
    "open",
    "start",
];

/// The variables and folders the providers take a sign-in from.
const SIGN_INS: [&str; 7] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CONFIG_DIR",
    "CODEX_ACCESS_TOKEN",
    "CODEX_HOME",
    "OPENAI_API_KEY",
];

/// The marker `User::with_clean_environment` sets: the commands of this user start from an empty
/// environment.
const CLEAN_ENVIRONMENT: &str = "FLOTTILLE_TEST_CLEAN_ENVIRONMENT";

/// The file `User::new` writes in the daemon's configuration folder, which no test's own folder
/// listing counts (`names_in`).
const HARNESS_FILE: &str = "tool-results.toml";

/// The other file `User::new` writes there: the vault's keyring is the daemon's memory, never the
/// user's own keyring (`docs/vault.md`).
const KEYRING_FILE: &str = "keyring.toml";

/// And this one: the daemon never checks for an update on its own (`docs/releases.md`,
/// "Updating"), so that no test reaches the network; `with_update_check` lets it.
const UPDATES_FILE: &str = "updates.toml";

/// The names in a folder, without the file the harness itself wrote there: what a test asks is
/// what the daemon left, not what `User::new` put.
pub fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| ![HARNESS_FILE, KEYRING_FILE, UPDATES_FILE].contains(&name.as_str()))
        .collect();
    names.sort();
    names
}

/// A process a test started (a fake server): killed and reaped when this is dropped, whether the
/// test passed, failed or panicked, and killed by the kernel when the test's thread ends or the
/// test binary is killed (`PR_SET_PDEATHSIG`), so that nothing outlives its test (#809).
///
/// The death signal follows the thread that started the process, which is the test's own: start
/// it from the test's thread, not from a scoped one that ends before the test.
pub struct Spawned(Child);

impl Spawned {
    pub fn new(mut command: Command) -> Spawned {
        let parent = std::process::id();
        // SAFETY: the closure runs in the forked child before `exec`. Its success path calls only
        // `prctl` and `getppid`, which are async-signal-safe: it allocates nothing and takes no
        // lock. `getppid` catches a parent that died between the fork and the `prctl`, whose
        // death signal would never come. Its second error path does allocate, though:
        // `io::Error::other` boxes the message, which is not async-signal-safe. That path runs
        // only when the test binary died in that same window, and then the child stops before
        // `exec`; glibc's malloc is usable in a forked child (it takes its locks around the
        // fork), so the allocation does not hang there. `io::Error::from_raw_os_error` would not
        // allocate, but it would lose the message that says why the spawn failed.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() as u32 != parent {
                    return Err(std::io::Error::other(
                        "the test ended as its process started",
                    ));
                }
                Ok(())
            });
        }
        Spawned(command.spawn().unwrap())
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl std::ops::Deref for Spawned {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Spawned {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

/// A throwaway user: its own home and XDG folders, and the daemon it starts, stopped on drop.
pub struct User {
    dir: TempDir,
    /// The runtime folder, `runtime` in `dir` by a symbolic link. A real one's parent is a
    /// folder nothing writes into (`/run/user`), and what the daemon runs may create nothing
    /// there (`docs/vault.md`, "The runtime folder"): `dir` is where the tests' programs write.
    _runtime: TempDir,
    /// More variables for every command, and so for the daemon the first one starts.
    env: Vec<(String, String)>,
    /// True once the user is dropped, its daemon stopped and its folder about to go: a client the
    /// test still holds (the window's link, which outlives the test function) must not start a
    /// daemon again in a folder that is removed (issue #54). Held by a connector for as long as
    /// it connects, so that the drop waits for it.
    closed: Arc<Mutex<bool>>,
    /// Stops this user's daemon when the test's binary ends without dropping the user (killed by
    /// a timeout): `flottille-fake-agent --after-eof`, run once the pipe the user holds closes.
    reaper: Option<Child>,
}

impl User {
    pub fn new() -> User {
        let dir = tempfile::Builder::new()
            .prefix("flottille-test")
            .tempdir()
            .unwrap();
        for sub in ["home", "state", "config", "data", "sign-in-bin"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(dir.path().join(sub))
                .unwrap();
        }
        let runtime = tempfile::Builder::new()
            .prefix("flottille-runtime")
            .tempdir()
            .unwrap();
        std::os::unix::fs::symlink(runtime.path(), dir.path().join("runtime")).unwrap();
        // The programs a sign-in runs are looked for there alone (`docs/agents.md`, "Signing in"):
        // a program the machine has installed is never run, and a test puts its fakes there.
        let programs = dir.path().join("sign-in-bin").display().to_string();
        let mut user = User {
            dir,
            _runtime: runtime,
            // A connection's browser runs in a network of its own, which passt's `pasta` gives a
            // way out (`docs/vault.md`, "The browser's network"): a test's stands on `true`, which
            // gives none, so that no test needs passt. A test of the real one names `pasta`.
            env: vec![
                ("FLOTTILLE_TEST_PROGRAMS".to_owned(), programs),
                (
                    "FLOTTILLE_TEST_PASTA".to_owned(),
                    program("true").display().to_string(),
                ),
            ],
            closed: Arc::new(Mutex::new(false)),
            reaper: None,
        };
        // The reaper holds nothing of the daemon's: it only waits for this end of its pipe to
        // close. It stops the daemon by the user's own folders, which is all `daemon stop` needs.
        let mut reaper = command_for(
            &fake_agent(),
            user.dir.path(),
            &[],
            &[
                "--after-eof",
                env!("CARGO_BIN_EXE_flottille"),
                "daemon",
                "stop",
            ],
        );
        reaper
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        user.reaper = Some(reaper.spawn().unwrap());
        // Long tool results are cut by default (`docs/tools.md`, "Long results"): only the tests
        // of that opt in, so that no other test's result is cut under it. The user's own setting,
        // in the daemon's temporary configuration folder: no switch of the daemon's environment.
        let file = user.dir.path().join("config/flottille");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&file)
            .unwrap();
        std::fs::write(file.join(HARNESS_FILE), "limit = \"off\"\n").unwrap();
        std::fs::write(file.join(KEYRING_FILE), "backend = \"memory\"\n").unwrap();
        std::fs::write(file.join(UPDATES_FILE), "check = \"never\"\n").unwrap();
        user
    }

    /// This user's daemon cuts long tool results as it does for everyone: every other test's
    /// daemon does not.
    pub fn with_cut_results(self) -> User {
        std::fs::remove_file(self.dir.path().join("config/flottille").join(HARNESS_FILE)).unwrap();
        self
    }

    /// This user's daemon checks for updates as it does for everyone (at its start, then every 6
    /// hours): every other test's daemon never does.
    pub fn with_update_check(self) -> User {
        std::fs::remove_file(self.dir.path().join("config/flottille").join(UPDATES_FILE)).unwrap();
        self
    }

    /// A variable for every command of this user, and so for its daemon.
    pub fn with_env(mut self, name: &str, value: &str) -> User {
        self.env.push((name.to_owned(), value.to_owned()));
        self
    }

    /// This user's daemon, and every command it starts, has the harness's environment alone: none
    /// of the developer's (forge tokens, compositor sockets, sign-ins), only their `PATH`. A run
    /// in Full inherits the daemon's environment as it is (`docs/tools.md`).
    pub fn with_clean_environment(self) -> User {
        self.with_env(CLEAN_ENVIRONMENT, "1")
    }

    /// This user's daemon keeps the items of its projects up to date (`docs/items.md`): every
    /// other test's daemon does not, so that no round of its own adds calls to the fakes.
    pub fn with_fetcher(self) -> User {
        self.with_env("FLOTTILLE_TEST_NO_FETCH", "")
    }

    /// A user whose agents are the fake ACP agent (`crates/flottille-fake-agent`), its folder
    /// `fake` in the user's folder.
    pub fn with_fake_agent() -> User {
        let user = User::new();
        std::fs::create_dir(user.path("fake")).unwrap();
        let fake = fake_agent();
        let dir = user.path("fake");
        user.with_env("FLOTTILLE_AGENT_COMMAND", fake.to_str().unwrap())
            .with_env("FLOTTILLE_FAKE_AGENT_DIR", dir.to_str().unwrap())
    }

    /// A user with three providers, Claude, OpenCode and Codex, the first two enabled, each
    /// standing on a fake ACP agent of its own: the per-provider test overrides name a link to the
    /// fake agent in the folders `fake-claude`, `fake-opencode` and `fake-codex`, which the fake
    /// agent takes for its own folder.
    pub fn with_fake_providers() -> User {
        let mut user = User::new();
        for provider in ["claude", "opencode", "codex"] {
            let dir = user.path(&format!("fake-{provider}"));
            std::fs::create_dir(&dir).unwrap();
            std::os::unix::fs::symlink(fake_agent(), dir.join("agent")).unwrap();
            let name = format!("FLOTTILLE_AGENT_COMMAND_{}", provider.to_uppercase());
            user = user.with_env(&name, dir.join("agent").to_str().unwrap());
        }
        user.write_providers("[opencode]\nenabled = true\n");
        user
    }

    /// A user whose Auto judge is OpenCode standing on a fake ACP agent of its own, in the folder
    /// `fake-judge`, every prompt of which plays `steps`: what the judge says, or `[{"hang":
    /// true}]` for a judge that never answers. Its `[judge]` is added to `providers.toml`, and
    /// OpenCode enabled there.
    pub fn with_judge(self, steps: Value) -> User {
        let dir = self.path("fake-judge");
        std::fs::create_dir(&dir).unwrap();
        // Its own folder, whatever folder the user's other fake agents are given.
        let agent = dir.join("agent");
        std::fs::write(
            &agent,
            format!(
                "#!/bin/sh\nFLOTTILLE_FAKE_AGENT_DIR='{}' exec '{}' \"$@\"\n",
                dir.display(),
                fake_agent().display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&agent, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        std::fs::write(
            dir.join("script.json"),
            json!({"turns": [{"when": "", "do": steps}]}).to_string(),
        )
        .unwrap();
        let file = self.path("config/flottille/providers.toml");
        let mut before = std::fs::read_to_string(&file).unwrap_or_default();
        // A judge runs on a provider that is enabled.
        if !before.contains("[opencode]") {
            before.push_str("\n[opencode]\nenabled = true\n");
        }
        self.write_providers(&format!("{before}\n[judge]\nprovider = \"opencode\"\n"));
        self.with_env("FLOTTILLE_AGENT_COMMAND_OPENCODE", agent.to_str().unwrap())
    }

    /// A user whose compactor is OpenCode standing on a fake ACP agent of its own, in the folder
    /// `fake-compactor`, every prompt of which plays `steps`; `settings` is the rest of
    /// `compaction.toml`. OpenCode is enabled.
    pub fn with_compactor(self, steps: Value, settings: &str) -> User {
        let dir = self.path("fake-compactor");
        std::fs::create_dir(&dir).unwrap();
        let agent = dir.join("agent");
        std::fs::write(
            &agent,
            format!(
                "#!/bin/sh\nFLOTTILLE_FAKE_AGENT_DIR='{}' exec '{}' \"$@\"\n",
                dir.display(),
                fake_agent().display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&agent, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        std::fs::write(
            dir.join("script.json"),
            json!({"turns": [{"when": "", "do": steps}]}).to_string(),
        )
        .unwrap();
        let file = self.path("config/flottille/providers.toml");
        let before = std::fs::read_to_string(&file).unwrap_or_default();
        if !before.contains("[opencode]") {
            self.write_providers(&format!("{before}\n[opencode]\nenabled = true\n"));
        }
        std::fs::write(
            self.path("config/flottille/compaction.toml"),
            format!("{settings}\n[compactor]\nprovider = \"opencode\"\n"),
        )
        .unwrap();
        self.with_env("FLOTTILLE_AGENT_COMMAND_OPENCODE", agent.to_str().unwrap())
    }

    /// What the fake compactor was asked, every message of every compaction.
    pub fn compactor_received(&self) -> Vec<Value> {
        records(&self.path("fake-compactor/received.jsonl"))
    }

    /// `with_judge`, the judge allowing every call it is asked about.
    pub fn with_allowing_judge(self) -> User {
        self.with_judge(
            json!([{"say": "{\"verdict\": \"allow\", \"reason\": \"It fits the task.\"}"}]),
        )
    }

    /// What the fake judge received: every message of every judgement.
    pub fn judge_received(&self) -> Vec<Value> {
        records(&self.path("fake-judge/received.jsonl"))
    }

    /// The user's `providers.toml`.
    pub fn write_providers(&self, text: &str) {
        let file = self.path("config/flottille/providers.toml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    /// The script of the fake agent standing for that provider (`with_fake_providers`).
    pub fn provider_script(&self, provider: &str, script: Value) {
        std::fs::write(
            self.path(&format!("fake-{provider}/script.json")),
            script.to_string(),
        )
        .unwrap();
    }

    /// What the fake agent standing for that provider received and did.
    pub fn provider_received(&self, provider: &str) -> Vec<Value> {
        records(&self.path(&format!("fake-{provider}/received.jsonl")))
    }

    /// A user whose agents run through Flottille's Claude bridge in front of the fake `claude`
    /// (`crates/flottille-fake-claude`): `bin/claude` first in `PATH`, its folder `claude`.
    pub fn with_fake_claude() -> User {
        let user = User::new();
        std::fs::create_dir(user.path("claude")).unwrap();
        std::fs::create_dir(user.path("bin")).unwrap();
        std::os::unix::fs::symlink(fake_claude(), user.path("bin/claude")).unwrap();
        // Signed in: Claude is ready only then, and only the file's presence is ever checked.
        std::fs::create_dir_all(user.path("home/.claude")).unwrap();
        std::fs::write(user.path("home/.claude/.credentials.json"), "{}").unwrap();
        // The test's own PATH after it, without `/usr/bin` forced in: a run on a PATH without
        // `node` shows that nothing needs it.
        let path = format!(
            "{}:{}",
            user.path("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let dir = user.path("claude");
        user.with_env("PATH", &path)
            .with_env("FLOTTILLE_FAKE_CLAUDE_DIR", dir.to_str().unwrap())
    }

    /// The programs this user's daemon signs in with (#1185, `docs/agents.md`, "Signing in"): the
    /// fakes named (`gh`, `glab`, `acli`, `bkt`, `claude`, `codex`), each a link in `programs`
    /// with a folder of its own, `sign-in-<name>`. No other program is found there, whatever the
    /// machine has installed.
    pub fn with_sign_in_programs(self, names: &[&str]) -> User {
        let mut user = self;
        for name in names {
            let fake = match *name {
                "gh" | "glab" => fake_gh(),
                "acli" => fake_acli(),
                "bkt" => fake_bkt(),
                "claude" => fake_claude(),
                "codex" => fake_codex(),
                other => panic!("no fake {other}"),
            };
            std::os::unix::fs::symlink(fake, user.path("sign-in-bin").join(name)).unwrap();
            let dir = user.path(&format!("sign-in-{name}"));
            std::fs::create_dir(&dir).unwrap();
            let variable = format!("FLOTTILLE_FAKE_{}_DIR", name.to_uppercase());
            user = user.with_env(&variable, dir.to_str().unwrap());
        }
        user
    }

    /// The script of the fake `name` a sign-in runs.
    pub fn sign_in_script(&self, name: &str, script: Value) {
        std::fs::write(
            self.path(&format!("sign-in-{name}/script.json")),
            script.to_string(),
        )
        .unwrap();
    }

    /// What the fake `name` did of its sign-in, the first first (`sign_in.rs` of the fake `gh`).
    pub fn sign_in_record(&self, name: &str) -> Vec<Value> {
        records(&self.path(&format!("sign-in-{name}/sign-in.jsonl")))
    }

    /// The user finishes the sign-in of the fake `name` in their browser.
    pub fn approve_sign_in(&self, name: &str) {
        std::fs::write(self.path(&format!("sign-in-{name}/approve")), "").unwrap();
    }

    /// The fake `claude`'s script.
    pub fn claude_script(&self, script: Value) {
        std::fs::write(self.path("claude/script.json"), script.to_string()).unwrap();
    }

    /// What the fake `claude` was started with and read, one entry per line of its record.
    pub fn claude_received(&self) -> Vec<Value> {
        match std::fs::read_to_string(self.path("claude/received.jsonl")) {
            Ok(text) => text
                .split_inclusive('\n')
                .filter(|line| line.ends_with('\n'))
                .map(|line| serde_json::from_str(line).unwrap())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// How each fake `claude` was started, the first first.
    pub fn claude_starts(&self) -> Vec<Value> {
        self.claude_received()
            .into_iter()
            .filter_map(|entry| entry.get("started").cloned())
            .collect()
    }

    /// A user whose agents run through Flottille's Codex bridge in front of the fake `codex`
    /// (`crates/flottille-fake-codex`): `bin/codex` first in `PATH`, its folder `codex`.
    pub fn with_fake_codex() -> User {
        let user = User::new();
        std::fs::create_dir(user.path("codex")).unwrap();
        std::fs::create_dir(user.path("bin")).unwrap();
        std::os::unix::fs::symlink(fake_codex(), user.path("bin/codex")).unwrap();
        let path = format!(
            "{}:{}",
            user.path("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let dir = user.path("codex");
        user.with_env("PATH", &path)
            .with_env("FLOTTILLE_FAKE_CODEX_DIR", dir.to_str().unwrap())
    }

    /// The fake `codex`'s script.
    pub fn codex_script(&self, script: Value) {
        std::fs::write(self.path("codex/script.json"), script.to_string()).unwrap();
    }

    /// What the fake `codex` was started with, read and was answered, one entry per line.
    pub fn codex_received(&self) -> Vec<Value> {
        records(&self.path("codex/received.jsonl"))
    }

    /// This user with the fake `gh` (`crates/flottille-fake-gh`) first in its `PATH`, its folder
    /// `gh`: no test ever calls GitHub nor uses anyone's sign-in.
    pub fn with_fake_gh(self) -> User {
        std::fs::create_dir(self.path("gh")).unwrap();
        std::fs::create_dir(self.path("gh-bin")).unwrap();
        std::os::unix::fs::symlink(fake_gh(), self.path("gh-bin/gh")).unwrap();
        let path = format!(
            "{}:{}",
            self.path("gh-bin").display(),
            self.env
                .iter()
                .rev()
                .find(|(name, _)| name == "PATH")
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
        );
        let dir = self.path("gh");
        self.with_env("PATH", &path)
            .with_env("FLOTTILLE_FAKE_GH_DIR", dir.to_str().unwrap())
    }

    /// The fake `gh`'s script, read again at each of its calls.
    pub fn gh_script(&self, script: Value) {
        std::fs::write(self.path("gh/script.json"), script.to_string()).unwrap();
    }

    /// Every call of the fake `gh`, the first first: `{"args", "env"}`.
    pub fn gh_received(&self) -> Vec<Value> {
        records(&self.path("gh/received.jsonl"))
    }

    /// This user with a fake `sudo`, `sudoedit`, `doas`, `pkexec`, `run0` and `su` first in its
    /// `PATH`: each writes a line in a marker file and exits 1. A test that sends one of them to
    /// the gate checks `escalations` is empty afterwards: a gate that let the call run would
    /// start a fake, never the program that asks the maintainer's machine for a password.
    pub fn with_fake_escalation(self) -> User {
        let bin = self.path("escalation-bin");
        std::fs::create_dir(&bin).unwrap();
        let marker = self.path("escalation-ran");
        for name in ["sudo", "sudoedit", "doas", "pkexec", "run0", "su"] {
            let program = bin.join(name);
            std::fs::write(
                &program,
                format!(
                    "#!/bin/sh\necho \"{name} $*\" >> '{}'\nexit 1\n",
                    marker.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!(
            "{}:{}",
            bin.display(),
            self.env
                .iter()
                .rev()
                .find(|(name, _)| name == "PATH")
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
        );
        // Looked up, never run: the real programs must be unreachable by name.
        for name in ["sudo", "sudoedit", "doas", "pkexec", "run0", "su"] {
            let found = path
                .split(':')
                .map(|folder| Path::new(folder).join(name))
                .find(|candidate| candidate.is_file());
            assert_eq!(found, Some(bin.join(name)), "{name} is not the fake one");
        }
        self.with_env("PATH", &path)
    }

    /// This user with a fake of every program that starts a process outside the daemon's tree,
    /// later or on a host, or reads a login, first in its `PATH`: each writes a line in a marker
    /// file and exits 1. A test that sends one of them to the gate checks `launches` is empty
    /// afterwards: a gate that let the call run would start a fake, never the machine's own user
    /// manager, tmux server, container engine or desktop.
    pub fn with_fake_launchers(self) -> User {
        let bin = self.path("launcher-bin");
        std::fs::create_dir(&bin).unwrap();
        let marker = self.path("launcher-ran");
        for name in LAUNCHERS {
            let program = bin.join(name);
            std::fs::write(
                &program,
                format!(
                    "#!/bin/sh\necho \"{name} $*\" >> '{}'\nexit 1\n",
                    marker.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!(
            "{}:{}",
            bin.display(),
            self.env
                .iter()
                .rev()
                .find(|(name, _)| name == "PATH")
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
        );
        // Looked up, never run: the real programs must be unreachable by name.
        for name in LAUNCHERS {
            let found = path
                .split(':')
                .map(|folder| Path::new(folder).join(name))
                .find(|candidate| candidate.is_file());
            assert_eq!(found, Some(bin.join(name)), "{name} is not the fake one");
        }
        self.with_env("PATH", &path)
    }

    /// What the fake launchers ran, the first first. Empty when the gate refused every call.
    pub fn launches(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("launcher-ran"))
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// The calls the fake escalating programs received, the first first: what ran of `sudo`,
    /// `doas`, `pkexec`, `run0` and `su`. Empty when the gate held every call.
    pub fn escalations(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("escalation-ran"))
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// This user with the fake `bkt` (`crates/flottille-fake-bkt`) first in its `PATH`, its folder
    /// `bkt`: no test ever calls Bitbucket nor uses anyone's sign-in.
    pub fn with_fake_bkt(self) -> User {
        std::fs::create_dir(self.path("bkt")).unwrap();
        std::fs::create_dir(self.path("bkt-bin")).unwrap();
        std::os::unix::fs::symlink(fake_bkt(), self.path("bkt-bin/bkt")).unwrap();
        let path = format!(
            "{}:{}",
            self.path("bkt-bin").display(),
            self.env
                .iter()
                .rev()
                .find(|(name, _)| name == "PATH")
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
        );
        let dir = self.path("bkt");
        self.with_env("PATH", &path)
            .with_env("FLOTTILLE_FAKE_BKT_DIR", dir.to_str().unwrap())
    }

    /// The fake `bkt`'s script, read again at each of its calls.
    pub fn bkt_script(&self, script: Value) {
        std::fs::write(self.path("bkt/script.json"), script.to_string()).unwrap();
    }

    /// Every call of the fake `bkt`, the first first: `{"args", "env"}`.
    pub fn bkt_received(&self) -> Vec<Value> {
        records(&self.path("bkt/received.jsonl"))
    }

    /// This user with the fake `acli` (`crates/flottille-fake-acli`) first in its `PATH`, its
    /// folder `acli`: no test ever calls Jira nor uses anyone's sign-in.
    pub fn with_fake_acli(self) -> User {
        std::fs::create_dir(self.path("acli")).unwrap();
        std::fs::create_dir(self.path("acli-bin")).unwrap();
        std::os::unix::fs::symlink(fake_acli(), self.path("acli-bin/acli")).unwrap();
        let path = format!(
            "{}:{}",
            self.path("acli-bin").display(),
            self.env
                .iter()
                .rev()
                .find(|(name, _)| name == "PATH")
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
        );
        let dir = self.path("acli");
        self.with_env("PATH", &path)
            .with_env("FLOTTILLE_FAKE_ACLI_DIR", dir.to_str().unwrap())
    }

    /// This user with a fake `ssh` first in its `PATH`, so that no test reads the user's SSH
    /// configuration nor reaches a server: `ssh -G <alias>` prints the resolved configuration of
    /// each alias of `aliases` (its `hostname` among other lines) and ends with 255 for any other
    /// name; git's own calls (`ssh <host> "git-receive-pack 'acme/api.git'"`) are served by the
    /// bare repository of the same name in the user's `remotes`, whatever the host. Every call's
    /// words are a line of `ssh-calls`.
    pub fn with_fake_ssh(self, aliases: &[(&str, &str)]) -> User {
        std::fs::create_dir(self.path("ssh-bin")).unwrap();
        let cases: String = aliases
            .iter()
            .map(|(alias, host)| format!("  {alias}) host={host} ;;\n"))
            .collect();
        // Its own programs from the test's `PATH`: a push's is the system's folders alone.
        let script = format!(
            "#!/bin/sh\n\
             PATH='{path}'\n\
             echo \"$*\" >> '{calls}'\n\
             if [ \"$1\" = -G ]; then\n\
             case \"$2\" in\n{cases}  *) echo \"ssh: Could not resolve hostname $2\" >&2; exit 255 ;;\n\
             esac\n\
             printf 'host %s\\nuser git\\nhostname %s\\nport 22\\nidentityfile ~/.ssh/id_work\\n' \"$2\" \"$host\"\n\
             exit 0\n\
             fi\n\
             for last; do :; done\n\
             service=${{last%% *}}\n\
             repository=$(basename \"$(echo \"${{last#* }}\" | tr -d \"'\")\")\n\
             exec git \"${{service#git-}}\" '{remotes}'/\"$repository\"\n",
            calls = self.path("ssh-calls").display(),
            remotes = self.path("remotes").display(),
            path = std::env::var("PATH").unwrap_or_default(),
        );
        let ssh = self.path("ssh-bin/ssh");
        std::fs::write(&ssh, script).unwrap();
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = self
            .env
            .iter()
            .rev()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
        let path = format!(
            "{}:{}",
            self.path("ssh-bin").display(),
            self.programs_but_ssh(&path).display()
        );
        self.with_env("PATH", &path)
    }

    /// A folder of links to every program of `path` but `ssh`, the first of each name: the
    /// daemon's git prefers an `ssh` owned by root to a fake one, and the machine's is.
    pub fn programs_but_ssh(&self, path: &str) -> PathBuf {
        let folder = self.path("programs");
        std::fs::create_dir(&folder).unwrap();
        for dir in std::env::split_paths(path) {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let name = entry.file_name();
                let link = folder.join(&name);
                if name != "ssh" && std::fs::symlink_metadata(&link).is_err() {
                    std::os::unix::fs::symlink(entry.path(), link).unwrap();
                }
            }
        }
        folder
    }

    /// The words of every call of the fake `ssh`, the first first.
    pub fn ssh_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("ssh-calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The fake `acli`'s script, read again at each of its calls.
    pub fn acli_script(&self, script: Value) {
        std::fs::write(self.path("acli/script.json"), script.to_string()).unwrap();
    }

    /// Every call of the fake `acli`, the first first: `{"args", "env"}`.
    pub fn acli_received(&self) -> Vec<Value> {
        records(&self.path("acli/received.jsonl"))
    }

    /// The fake agent's script (`crates/flottille-fake-agent`).
    pub fn fake_script(&self, script: Value) {
        std::fs::write(self.path("fake/script.json"), script.to_string()).unwrap();
    }

    /// What the fake agent received and did, one entry per line of its record. A line it is
    /// still writing is left for the next read.
    pub fn received(&self) -> Vec<Value> {
        records(&self.path("fake/received.jsonl"))
    }

    /// The requests of one method the fake agent received, oldest first.
    pub fn received_requests(&self, method: &str) -> Vec<Value> {
        self.received()
            .into_iter()
            .filter(|entry| entry["received"]["method"] == method)
            .map(|entry| entry["received"].clone())
            .collect()
    }

    pub fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    pub fn command(&self, args: &[&str]) -> Command {
        command_of(self.dir.path(), &self.env, args)
    }

    /// A command of this user run through another path to the program: a link to it, for
    /// instance, under the name it will answer to.
    pub fn command_through(&self, program: &Path, args: &[&str]) -> Command {
        command_for(program, self.dir.path(), &self.env, args)
    }

    /// Connects to this user's daemon as a client that starts it when nobody listens (the
    /// command line's `connect_or_start`): `flottille status` starts it, then the socket is
    /// opened. Usable from any thread; once the user is dropped it fails and starts nothing.
    pub fn connector(&self) -> impl Fn() -> std::io::Result<UnixStream> + Send + Sync + 'static {
        let dir = self.dir.path().to_owned();
        let env = self.env.clone();
        let closed = self.closed.clone();
        move || {
            let closed = closed.lock().unwrap();
            if *closed {
                return Err(std::io::Error::other("the user was dropped"));
            }
            let output = command_of(&dir, &env, &["status", "--json"]).output()?;
            if !output.status.success() {
                return Err(std::io::Error::other(
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                ));
            }
            UnixStream::connect(dir.join("runtime/flottille/daemon.sock"))
        }
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    pub fn status(&self) -> Value {
        let output = self.run(&["status", "--json"]);
        assert!(
            output.status.success(),
            "status failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// A command that reads `input` on its standard input, then ends it.
    pub fn run_with_input(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// One request over the socket, to a daemon the first command starts: its result, or the
    /// code and the message of its error.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, (String, String)> {
        self.call_within(method, params, Duration::from_secs(10))
    }

    /// `call`, for a method that may take longer than its 10 seconds to answer: each wait on the
    /// socket lasts `within`, and a daemon that is still working is waited for again.
    pub fn call_within(
        &self,
        method: &str,
        params: Value,
        within: Duration,
    ) -> Result<Value, (String, String)> {
        self.status();
        let mut stream = UnixStream::connect(self.path("runtime/flottille/daemon.sock")).unwrap();
        stream.set_read_timeout(Some(within)).unwrap();
        let request = serde_json::json!({"id": "1", "method": method, "params": params});
        writeln!(stream, "{request}").unwrap();
        let line = read_line_patiently(stream);
        let answer: Value = serde_json::from_str(&line).unwrap();
        match answer.get("error") {
            Some(error) => Err((
                error["code"].as_str().unwrap().to_owned(),
                error["message"].as_str().unwrap().to_owned(),
            )),
            None => Ok(answer["result"].clone()),
        }
    }

    /// `agent.resume`, once the agent's last process is gone: refused while it still stops.
    pub fn resume(&self, agent: &str) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match self.call("agent.resume", serde_json::json!({"agent": agent})) {
                Err((_, message)) if message.contains("still stopping") => {
                    assert!(std::time::Instant::now() < deadline, "{message}");
                    std::thread::sleep(Duration::from_millis(50));
                }
                answer => return answer.unwrap(),
            }
        }
    }

    /// A throwaway git repository in `path`, which is made with its parents.
    pub fn git_init(&self, path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        self.git(path, &["init", "--quiet", "--initial-branch=main"]);
    }

    /// The user's `git` in `dir`, with a throwaway identity and none of the machine's settings.
    pub fn git(&self, dir: &Path, args: &[&str]) {
        self.git_output(dir, args);
    }

    /// The same, its standard output.
    pub fn git_output(&self, dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("HOME", self.path("home"))
            .envs(GIT_ENV)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Where the project's file is.
    pub fn project_file(&self, name: &str) -> PathBuf {
        self.path(&format!("config/flottille/projects/{name}.toml"))
    }

    /// A project's file written by hand, as a user would.
    pub fn write_project_file(&self, name: &str, text: &str) {
        std::fs::create_dir_all(self.project_file(name).parent().unwrap()).unwrap();
        std::fs::write(self.project_file(name), text).unwrap();
    }

    pub fn watch(&self, args: &[&str]) -> Watch {
        let mut child = self
            .command(&[&["watch"], args].concat())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Watch { child, lines }
    }
}

/// The records of a fake agent, one per line. A line it is still writing is left for the next
/// read.
/// One line from `stream`, whose read timeout only says how often to look again: a daemon busy
/// with a loaded machine answers late, and one that is gone closes the socket, which ends the
/// read at once.
pub fn read_line_patiently(stream: UnixStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        match reader.read_line(&mut line) {
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            read => {
                read.unwrap();
                return line;
            }
        }
    }
}

fn records(file: &Path) -> Vec<Value> {
    match std::fs::read_to_string(file) {
        Ok(text) => text
            .split_inclusive('\n')
            .filter(|line| line.ends_with('\n'))
            .map(|line| serde_json::from_str(line).unwrap())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// A command of the user whose folder is `dir`.
fn command_of(dir: &Path, env: &[(String, String)], args: &[&str]) -> Command {
    command_for(Path::new(env!("CARGO_BIN_EXE_flottille")), dir, env, args)
}

fn command_for(program: &Path, dir: &Path, env: &[(String, String)], args: &[&str]) -> Command {
    let mut command = Command::new(program);
    if env.iter().any(|(name, _)| name == CLEAN_ENVIRONMENT) {
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
    }
    command
        .args(args)
        .env("HOME", dir.join("home"))
        .env("XDG_RUNTIME_DIR", dir.join("runtime"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        // Codex's machine settings are the test's own, not the machine's `/etc/codex`.
        .env("FLOTTILLE_CODEX_MACHINE_SETTINGS", dir.join("etc-codex"))
        // No way to the machine's own session: its tmux server.
        .env("TMUX_TMPDIR", dir.join("runtime"))
        // A session bus of the test's own, where a machine's is: the daemon keeps it from what
        // it runs (`docs/vault.md`), not the harness.
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", dir.join("runtime/bus").display()),
        )
        // X11's sockets where the machine's are: the test's own folder, never `/tmp/.X11-unix`.
        .env("FLOTTILLE_X11_SOCKETS", dir.join("x11"))
        // No wait for the checks a pull request change may start, unless a test asks for it.
        .env("FLOTTILLE_CHECKS_WAIT_MS", "0")
        // No round of the items unless a test asks for it (`with_fetcher`).
        .env("FLOTTILLE_TEST_NO_FETCH", "1")
        // Never GitHub's releases: nothing listens there. A test of the update serves its own
        // (`with_env("FLOTTILLE_RELEASES", ..)`).
        .env("FLOTTILLE_RELEASES", "http://127.0.0.1:9/repos/none")
        .env_remove("TMUX")
        // Never the machine's SSH agent: a test that lends one sets its own (`with_env`).
        .env_remove("SSH_AUTH_SOCK")
        // The account's home, which `ssh` reads `~` as: the test's, never the machine's
        // `~/.ssh` (a test of another sets its own).
        .env("FLOTTILLE_TEST_ACCOUNT_HOME", dir.join("home"));
    // No display of the machine's: a test that wants one gives its own.
    for name in ["WAYLAND_DISPLAY", "DISPLAY", "XAUTHORITY"] {
        command.env_remove(name);
    }
    // No sign-in of the machine: a provider is signed in only where a test says so, and the
    // daemon picks no Auto judge on the strength of the developer's own.
    for name in SIGN_INS {
        command.env_remove(name);
    }
    command
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null());
    // The daemon runs git: never with the machine's settings, unless a test gives its own
    // (`with_env`).
    for (name, value) in GIT_ENV {
        if !env.iter().any(|(given, _)| given == name) {
            command.env(name, value);
        }
    }
    command
}

impl Drop for User {
    fn drop(&mut self) {
        *self.closed.lock().unwrap() = true;
        // Ended first: the stop below is this drop's, and the reaper's own is for a test that
        // never got here.
        if let Some(mut reaper) = self.reaper.take() {
            let _ = reaper.kill();
            let _ = reaper.wait();
        }
        let _ = self.run(&["daemon", "stop"]);
    }
}

/// A running `flottille watch`, its output read line by line.
pub struct Watch {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Watch {
    /// The events up to the first one `until` accepts, that one included.
    pub fn until(&self, until: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        loop {
            let event = self.next_event();
            let done = until(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    /// Every event that arrives within `wait`.
    pub fn within(&self, wait: Duration) -> Vec<Value> {
        let deadline = std::time::Instant::now() + wait;
        let mut seen = Vec::new();
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match self.lines.recv_timeout(left) {
                Ok(line) => seen.push(serde_json::from_str(&line).unwrap()),
                Err(_) => break,
            }
        }
        seen
    }

    pub fn next_event(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(10))
            .expect("no event within 10 s");
        serde_json::from_str(&line).unwrap()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The fake agent's binary, built beside Flottille's by `cargo test` (or `cargo build`) at the
/// workspace's root.
pub fn fake_agent() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-agent");
    assert!(
        path.exists(),
        "{} is missing: run the tests from the workspace's root, or `cargo build -p flottille-fake-agent` first",
        path.display()
    );
    path
}

/// A program of the test's own PATH, as a shell would find it.
pub fn program(name: &str) -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("{name} is not on PATH"))
}

/// The fake `claude`'s binary, built beside Flottille's as the fake agent is.
pub fn fake_claude() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-claude");
    assert!(
        path.exists(),
        "{} is missing: run the tests from the workspace's root, or `cargo build -p flottille-fake-claude` first",
        path.display()
    );
    path
}

/// The fake `gh`'s binary, built beside Flottille's as the fake agent is.
pub fn fake_gh() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-gh");
    assert!(
        path.exists(),
        "{} is missing: run the tests from the workspace's root, or `cargo build -p flottille-fake-gh` first",
        path.display()
    );
    path
}

/// The fake development server's binary (`crates/flottille-fake-server`), built beside
/// Flottille's as the fake agent is.
pub fn fake_server() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-server");
    assert!(
        path.exists(),
        "{} is missing: run the tests from the workspace's root, or `cargo build -p flottille-fake-server` first",
        path.display()
    );
    path
}

/// The fake `bkt`'s binary, built beside Flottille's as the fake agent is.
pub fn fake_bkt() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-bkt");
    assert!(
        path.exists(),
        "{} is missing: run the tests from the workspace's root, or `cargo build -p flottille-fake-bkt` first",
        path.display()
    );
    path
}

/// The fake pnpm launcher's binary (`crates/flottille-fake-pnpm`), built beside Flottille's as
/// the fake agent is.
pub fn fake_pnpm() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-pnpm");
    assert!(
        path.exists(),
        "{} is missing: run the tests from the workspace's root, or `cargo build -p flottille-fake-pnpm` first",
        path.display()
    );
    path
}

/// The fake `acli`'s binary, built beside Flottille's as the fake agent is.
pub fn fake_acli() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-acli");
    assert!(
        path.exists(),
        "{} is missing: run the tests from the workspace's root, or `cargo build -p flottille-fake-acli` first",
        path.display()
    );
    path
}

/// The fake `codex`'s binary, built beside Flottille's as the fake agent is.
pub fn fake_codex() -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_flottille")).with_file_name("flottille-fake-codex");
    assert!(
        path.exists(),
        "{} is missing: run the tests from the workspace's root, or `cargo build -p flottille-fake-codex` first",
        path.display()
    );
    path
}

pub fn mode(path: PathBuf) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Every file and folder under `folder` with its size and modification time: two snapshots are
/// equal when nothing was written, changed or removed there.
pub fn snapshot(folder: &Path) -> Vec<String> {
    fn walk(folder: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            out.push(format!(
                "{} {} {:?}",
                path.strip_prefix(root).unwrap().display(),
                metadata.len(),
                metadata.modified().unwrap()
            ));
            if metadata.is_dir() {
                walk(&path, root, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(folder, folder, &mut out);
    out.sort();
    out
}

/// A time `seconds` after 2026-03-01T10:00:00, as the forges print it.
fn stamp(seconds: u64) -> String {
    let at = 10 * 3600 + seconds;
    format!(
        "2026-03-{:02}T{:02}:{:02}:{:02}Z",
        1 + at / 86_400,
        at % 86_400 / 3600,
        at % 3600 / 60,
        at % 60
    )
}

/// A run of `gh run list --json conclusion,startedAt,updatedAt` that took `seconds`.
pub fn gh_run(conclusion: &str, seconds: u64) -> Value {
    json!({"status": "completed", "conclusion": conclusion,
        "startedAt": stamp(0), "updatedAt": stamp(seconds)})
}

/// A pipeline of `bkt pipeline list --json` that took `seconds`, ended `result` (`SUCCESSFUL`,
/// `FAILED`, `ERROR`, `STOPPED`) or still running (`state.name` `IN_PROGRESS`) when it is empty;
/// its times as Bitbucket prints them, with microseconds and an offset.
pub fn bkt_pipeline(result: &str, seconds: u64) -> Value {
    let stamp = |seconds| stamp(seconds).replace('Z', ".500000+00:00");
    if result.is_empty() {
        return json!({"build_number": 1, "state": {"name": "IN_PROGRESS", "result": {"name": ""}},
            "target": {"ref": {"name": "main"}}, "created_on": stamp(0), "completed_on": ""});
    }
    json!({"build_number": 1, "state": {"name": "COMPLETED", "result": {"name": result}},
        "target": {"ref": {"name": "main"}}, "created_on": stamp(0), "completed_on": stamp(seconds)})
}

/// The ABI of the kernel's Landlock, 0 where it has none.
pub fn landlock_abi() -> i64 {
    // SAFETY: with a null attribute and the version flag, the call only returns a number.
    unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<u8>(),
            0usize,
            1u32,
        )
    }
    .max(0)
}

/// Whether the kernel gives this user a user namespace in which it may mount: what the private
/// mount namespace of a confined command needs (`mounts.rs`). A distribution may refuse either
/// (Ubuntu's AppArmor lets the namespace be made, then refuses its mounts).
pub fn user_namespaces() -> bool {
    const PROBE: &str = "import ctypes, os, sys\n\
        libc = ctypes.CDLL(None, use_errno=True)\n\
        uid, gid = os.getuid(), os.getgid()\n\
        if libc.unshare(0x10000000 | 0x00020000) != 0: sys.exit(1)\n\
        open('/proc/self/setgroups', 'w').write('deny')\n\
        open('/proc/self/uid_map', 'w').write(f'{uid} {uid} 1')\n\
        open('/proc/self/gid_map', 'w').write(f'{gid} {gid} 1')\n\
        sys.exit(0 if libc.mount(b'tmpfs', b'/', b'tmpfs', 0, None) == 0 else 1)\n";
    Command::new("python3")
        .args(["-c", PROBE])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
