//! What the integration tests share: a throwaway user and the daemon it starts.

// Each test file uses its own part of it.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

/// A throwaway user: its own home and XDG folders, and the daemon it starts, stopped on drop.
pub struct User {
    dir: TempDir,
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
        User { dir }
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
            .stdin(Stdio::null());
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
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("HOME", self.path("home"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
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
