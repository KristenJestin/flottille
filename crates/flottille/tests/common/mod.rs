//! What the integration tests share: a throwaway user and the daemon it starts.

// Each test file uses its own part of it.
#![allow(dead_code)]

pub mod mcp;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;
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

/// A throwaway user: its own home and XDG folders, and the daemon it starts, stopped on drop.
pub struct User {
    dir: TempDir,
    /// More variables for every command, and so for the daemon the first one starts.
    env: Vec<(String, String)>,
}

impl User {
    pub fn new() -> User {
        let dir = tempfile::Builder::new()
            .prefix("flottille-test")
            .tempdir()
            .unwrap();
        for sub in ["home", "runtime", "state", "config", "data"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(dir.path().join(sub))
                .unwrap();
        }
        User {
            dir,
            env: Vec::new(),
        }
    }

    /// A variable for every command of this user, and so for its daemon.
    pub fn with_env(mut self, name: &str, value: &str) -> User {
        self.env.push((name.to_owned(), value.to_owned()));
        self
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

    /// A user whose agents run through Flottille's Claude bridge in front of the fake `claude`
    /// (`crates/flottille-fake-claude`): `bin/claude` first in `PATH`, its folder `claude`.
    pub fn with_fake_claude() -> User {
        let user = User::new();
        std::fs::create_dir(user.path("claude")).unwrap();
        std::fs::create_dir(user.path("bin")).unwrap();
        std::os::unix::fs::symlink(fake_claude(), user.path("bin/claude")).unwrap();
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

    /// The fake agent's script (`crates/flottille-fake-agent`).
    pub fn fake_script(&self, script: Value) {
        std::fs::write(self.path("fake/script.json"), script.to_string()).unwrap();
    }

    /// What the fake agent received and did, one entry per line of its record. A line it is
    /// still writing is left for the next read.
    pub fn received(&self) -> Vec<Value> {
        match std::fs::read_to_string(self.path("fake/received.jsonl")) {
            Ok(text) => text
                .split_inclusive('\n')
                .filter(|line| line.ends_with('\n'))
                .map(|line| serde_json::from_str(line).unwrap())
                .collect(),
            Err(_) => Vec::new(),
        }
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_flottille"));
        command
            .args(args)
            .env("HOME", self.path("home"))
            .env("XDG_RUNTIME_DIR", self.path("runtime"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null());
        // The daemon runs git: never with the machine's settings.
        for (name, value) in GIT_ENV {
            command.env(name, value);
        }
        command
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
        self.status();
        let mut stream = UnixStream::connect(self.path("runtime/flottille/daemon.sock")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let request = serde_json::json!({"id": "1", "method": method, "params": params});
        writeln!(stream, "{request}").unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        let answer: Value = serde_json::from_str(&line).unwrap();
        match answer.get("error") {
            Some(error) => Err((
                error["code"].as_str().unwrap().to_owned(),
                error["message"].as_str().unwrap().to_owned(),
            )),
            None => Ok(answer["result"].clone()),
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

impl Drop for User {
    fn drop(&mut self) {
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
