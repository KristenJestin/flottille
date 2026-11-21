//! A push the gate let through runs outside the sandbox, as if the user had typed it: the user's
//! credential helper, SSH agent and settings, no hook and none of the project's variables
//! (`docs/commands.md`, "The environment", and `docs/vault.md`, "The runtime folder"). Every
//! other run reaches no credential. The agent is a listening socket of the test's runtime folder,
//! `ssh` a fake that says whether it could connect to it and serves git from a throwaway bare
//! repository, an HTTP remote a small server of the test's over `git http-backend` that asks for
//! a sign-in, and the agents and the judge are fake ACP agents.

use crate::common;

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp, server_of};
use common::{Spawned, User, landlock_abi};
use flottille_core::store::Store;
use serde_json::json;

/// A user whose daemon's `SSH_AUTH_SOCK` is `runtime/ssh-agent.sock` (listening), with a session
/// bus socket at `runtime/bus` (listening too) and the fake `ssh` of `fake_ssh` first in its
/// `PATH`.
fn user() -> (User, UnixListener, UnixListener) {
    user_with("runtime/ssh-agent.sock")
}

/// `user`, the agent listening at `agent`.
fn user_with(agent: &str) -> (User, UnixListener, UnixListener) {
    let user = User::with_fake_agent().with_allowing_judge();
    let at = user.path(agent);
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    let listening = UnixListener::bind(&at).unwrap();
    let bus = UnixListener::bind(user.path("runtime/bus")).unwrap();
    let user = fake_ssh(user).with_env("SSH_AUTH_SOCK", at.to_str().unwrap());
    (user, listening, bus)
}

/// The SSH agent's socket, as the daemon's `SSH_AUTH_SOCK` names it.
fn socket(user: &User) -> PathBuf {
    socket_at(user, "runtime/ssh-agent.sock")
}

fn socket_at(user: &User, agent: &str) -> PathBuf {
    user.path(agent)
}

/// A fake `ssh` first in the user's `PATH`: each call adds to `ssh-said` whether it reached the
/// socket its `SSH_AUTH_SOCK` names (`reached <path>`, `refused <path>`, `no agent`), the value
/// of `FLOTTILLE_PLANTED` it was given (`planted=…`), the session bus's address (`dbus=…`),
/// whether it could list the runtime folder (`runtime listed`, `runtime unlisted`) and connect to its bus (`bus reached`, `bus refused`),
/// its words (`args …`), and what git asked it to run on the remote (`asked git-upload-pack '…'`), and serves git's
/// own call from
/// `remotes/<name>`, whatever the host.
fn fake_ssh(user: User) -> User {
    std::fs::create_dir(user.path("ssh-bin")).unwrap();
    let said = user.path("ssh-said");
    let connect = "python3 -c 'import socket, sys; \
                   socket.socket(socket.AF_UNIX).connect(sys.argv[1])'";
    // Its own programs from the test's `PATH`: a push's is the system's folders alone.
    let script = format!(
        "#!/bin/sh\n\
         PATH='{path}'\n\
         if [ -z \"$SSH_AUTH_SOCK\" ]; then echo 'no agent' >> '{said}'\n\
         elif {connect} \"$SSH_AUTH_SOCK\" 2>/dev/null; then echo \"reached $SSH_AUTH_SOCK\" >> '{said}'\n\
         else echo \"refused $SSH_AUTH_SOCK\" >> '{said}'; fi\n\
         echo \"planted=$FLOTTILLE_PLANTED\" >> '{said}'\n\
         echo \"dbus=$DBUS_SESSION_BUS_ADDRESS\" >> '{said}'\n\
         if ls '{runtime}' >/dev/null 2>&1; then echo 'runtime listed' >> '{said}'\n\
         else echo 'runtime unlisted' >> '{said}'; fi\n\
         if {connect} '{runtime}/bus' 2>/dev/null; then echo 'bus reached' >> '{said}'\n\
         else echo 'bus refused' >> '{said}'; fi\n\
         echo \"args $*\" >> '{said}'\n\
         for last; do :; done\n\
         echo \"asked $last\" >> '{said}'\n\
         service=${{last%% *}}\n\
         repository=$(basename \"$(echo \"${{last#* }}\" | tr -d \"'\")\")\n\
         exec git \"${{service#git-}}\" '{remotes}'/\"$repository\"\n",
        said = said.display(),
        remotes = user.path("remotes").display(),
        runtime = user.path("runtime").display(),
        path = std::env::var("PATH").unwrap_or_default(),
    );
    let ssh = user.path("ssh-bin/ssh");
    std::fs::write(&ssh, script).unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        user.path("ssh-bin").display(),
        user.programs_but_ssh(&std::env::var("PATH").unwrap_or_default())
            .display()
    );
    user.with_env("PATH", &path)
}

/// What the fake `ssh` said, one line each.
fn ssh_said(user: &User) -> Vec<String> {
    std::fs::read_to_string(user.path("ssh-said"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// A project `app` over a throwaway repository whose remote `origin` is `url` (served by
/// `remotes/app.git`), and the session of a root agent in the project folder.
fn session(user: &User, url: &str) -> Mcp {
    project(user, url);
    agent(user)
}

/// A project `app` over a throwaway repository whose remote `origin` is `url`, served by
/// `remotes/app.git`, which has `main`.
fn project(user: &User, url: &str) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.git(&folder, &["remote", "set-url", "origin", url]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
}

/// The session of a new root agent of `app`, which reads the project's place as it is now.
fn agent(user: &User) -> Mcp {
    let opened = user.received_requests("session/new").len();
    user.call(
        "agent.run",
        json!({"project": "app", "message": "push the work, then stop"}),
    )
    .unwrap();
    let (url, authorization) = server_of(user, opened);
    Mcp::initialize(&url, &authorization)
}

const SSH_REMOTE: &str = "git@forge.invalid:acme/app.git";

/// Whether the throwaway remote has the branch.
fn remote_has(user: &User, branch: &str) -> bool {
    std::process::Command::new("git")
        .arg("--git-dir")
        .arg(user.path("remotes/app.git"))
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .unwrap()
        .status
        .success()
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

/// A user's answer to a held call: allowed, what the agent is handed for it.
fn allowed(user: &User, answer: Answer) -> Answer {
    let number: u64 = answer
        .text
        .strip_prefix("Waiting for the user's approval, request #")
        .and_then(|rest| rest.split_once('.'))
        .map(|(number, _)| number.parse().unwrap())
        .unwrap_or_else(|| panic!("not held: {}", answer.text));
    user.call("approval.approve", json!({"number": number}))
        .unwrap();
    let store = || Store::open_read_only(&user.path("state/flottille/flottille.db")).unwrap();
    let id = format!("approval-{number}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while store().delivery(&id).unwrap().is_none() {
        assert!(Instant::now() < deadline, "no answer to request #{number}");
        std::thread::sleep(Duration::from_millis(50));
    }
    let text = store().delivery(&id).unwrap().unwrap().body;
    Answer {
        error: !text.contains("and Flottille has now done it"),
        text,
    }
}

/// A line of the agent's: run at once, or held and allowed by the user.
fn line(user: &User, mcp: &Mcp, line: &str) -> Answer {
    let answer = mcp.call("run", json!({"line": line, "timeout": 30}));
    if answer.text.starts_with("Waiting for the user's approval") {
        allowed(user, answer)
    } else {
        answer
    }
}

/// An executable script at `path`.
fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// An HTTP server of `remotes` on loopback, through `git http-backend`, that answers a request
/// without a sign-in with 401 (as a forge does): the process and the URL of `app.git`.
fn http_remote(user: &User) -> (Spawned, String) {
    let port_file = user.path("http-port");
    let server = Spawned::new({
        let mut command = std::process::Command::new("python3");
        command
            .args(["-c", HTTP_REMOTE])
            .arg(user.path("remotes"))
            .arg(&port_file)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        command
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while !port_file.exists() {
        assert!(Instant::now() < deadline, "the HTTP remote never listened");
        std::thread::sleep(Duration::from_millis(20));
    }
    let port = std::fs::read_to_string(&port_file).unwrap();
    (server, format!("http://127.0.0.1:{port}/app.git"))
}

const HTTP_REMOTE: &str = r#"import base64, os, subprocess, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
root, port_file = sys.argv[1], sys.argv[2]
class Git(BaseHTTPRequestHandler):
    def do_GET(self):
        self.serve()
    def do_POST(self):
        self.serve()
    def serve(self):
        signed = self.headers.get("Authorization", "")
        if not signed.startswith("Basic "):
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="forge"')
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        user = base64.b64decode(signed[6:]).decode().split(":", 1)[0]
        path, _, query = self.path.partition("?")
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        env = dict(os.environ, GIT_PROJECT_ROOT=root, GIT_HTTP_EXPORT_ALL="1",
                   PATH_INFO=path, QUERY_STRING=query, REQUEST_METHOD=self.command,
                   CONTENT_TYPE=self.headers.get("Content-Type", ""),
                   CONTENT_LENGTH=str(len(body)), REMOTE_USER=user, REMOTE_ADDR="127.0.0.1")
        if self.headers.get("Content-Encoding"):
            env["HTTP_CONTENT_ENCODING"] = self.headers["Content-Encoding"]
        if self.headers.get("Git-Protocol"):
            env["GIT_PROTOCOL"] = self.headers["Git-Protocol"]
        out = subprocess.run(["git", "http-backend"], input=body, env=env,
                             capture_output=True).stdout
        head, _, rest = out.partition(b"\r\n\r\n")
        status, headers = 200, []
        for line in head.split(b"\r\n"):
            name, _, value = line.decode().partition(":")
            if name.lower() == "status":
                status = int(value.split()[0])
            elif name:
                headers.append((name, value.strip()))
        self.send_response(status)
        for name, value in headers:
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(rest)))
        self.end_headers()
        self.wfile.write(rest)
    def log_message(self, *args):
        pass
server = HTTPServer(("127.0.0.1", 0), Git)
with open(port_file + ".part", "w") as file:
    file.write(str(server.server_port))
os.rename(port_file + ".part", port_file)
server.serve_forever()
"#;

/// A credential helper in the user's own git settings (`~/.gitconfig`, which the daemon reads in
/// place of none), as `gh auth setup-git` writes one: it adds each action it is asked for to
/// `helper-said` and gives an invented sign-in.
fn credential_helper(user: User) -> User {
    let helper = user.path("helper/sign-in");
    script(
        &helper,
        &format!(
            "echo \"$1\" >> '{said}'\n\
             echo \"agent=$SSH_AUTH_SOCK\" >> '{said}'\n\
             if [ \"$1\" = get ]; then echo username=octo; echo password=invented-password; fi\n",
            said = user.path("helper-said").display()
        ),
    );
    std::fs::create_dir_all(user.path("home")).unwrap();
    std::fs::write(
        user.path("home/.gitconfig"),
        format!("[credential]\n\thelper = {}\n", helper.display()),
    )
    .unwrap();
    let settings = user.path("home/.gitconfig");
    user.with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap())
}

fn helper_said(user: &User) -> Vec<String> {
    std::fs::read_to_string(user.path("helper-said"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn an_allowed_push_signs_in_with_the_users_credential_helper() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    let mcp = session(&user, &url);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/allowed");

    assert!(
        pushed.text.contains("exited with code 0"),
        "{}",
        pushed.text
    );
    assert!(remote_has(&user, "allowed"), "{}", pushed.text);
    assert!(
        helper_said(&user).contains(&"get".to_owned()),
        "{:?}",
        helper_said(&user)
    );
}

#[test]
fn a_checked_push_signs_in_for_its_dry_run_too() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    let mcp = session(&user, &url);
    // A branch format makes the checks read git's dry run of the push before it goes.
    user.call(
        "project.set_conventions",
        json!({"name": "app", "branch_format": "^(feat|fix)/[a-z-]+$"}),
    )
    .unwrap();
    let mcp = {
        drop(mcp);
        agent(&user)
    };

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/fix/signed-in");

    assert!(
        pushed.text.contains("exited with code 0"),
        "{}",
        pushed.text
    );
    assert!(remote_has(&user, "fix/signed-in"), "{}", pushed.text);
}

#[test]
fn an_allowed_push_reaches_the_users_ssh_agent() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/allowed"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(remote_has(&user, "allowed"));
    let reached = format!("reached {}", socket(&user).display());
    assert!(ssh_said(&user).contains(&reached), "{:?}", ssh_said(&user));
}

#[test]
fn an_allowed_push_runs_no_planted_ssh_command() {
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);
    let planted = user.path("planted/ssh");
    script(
        &planted,
        &format!(
            "echo ran >> '{}'\nexit 1\n",
            user.path("planted-ran").display()
        ),
    );
    // As a run could write it in the repository's settings.
    user.git(
        &user.path("work/app"),
        &["config", "core.sshCommand", planted.to_str().unwrap()],
    );
    let mcp = agent(&user);

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/planted"}),
    ));

    assert!(!user.path("planted-ran").exists(), "the planted ssh ran");
    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(remote_has(&user, "planted"));
}

/// A script at `path` that notes it ran in `<path>-ran`, then is the fake `ssh` of the user.
fn marker(user: &User, path: &Path) {
    script(
        path,
        &format!(
            "echo ran >> '{}-ran'\nexec '{}' \"$@\"\n",
            path.display(),
            user.path("ssh-bin/ssh").display()
        ),
    );
}

#[test]
fn an_allowed_push_runs_the_user_s_global_ssh_command() {
    let (user, _agent, _bus) = user();
    // In `~/.ssh`, which no run can change.
    let own = user.path("home/.ssh/own-ssh");
    marker(&user, &own);
    // The user's own, for several accounts of a forge: in their global settings (the test's,
    // which the harness otherwise empties).
    std::fs::create_dir_all(user.path("home")).unwrap();
    let global = user.path("home/.gitconfig");
    std::fs::write(
        &global,
        format!("[core]\n\tsshCommand = {}\n", own.display()),
    )
    .unwrap();
    let user = user.with_env("GIT_CONFIG_GLOBAL", global.to_str().unwrap());
    let mcp = session(&user, SSH_REMOTE);

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/own"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(
        user.path("home/.ssh/own-ssh-ran").exists(),
        "the user's ssh did not run"
    );
    assert!(remote_has(&user, "own"));
}

#[test]
fn an_allowed_push_keeps_the_daemon_s_git_ssh_command() {
    let (user, _agent, _bus) = user();
    let own = user.path("home/.ssh/own-ssh");
    marker(&user, &own);
    let user = user.with_env("GIT_SSH_COMMAND", own.to_str().unwrap());
    let mcp = session(&user, SSH_REMOTE);

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/own"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(
        user.path("home/.ssh/own-ssh-ran").exists(),
        "the user's ssh did not run"
    );
    assert!(remote_has(&user, "own"));
}

#[test]
fn an_allowed_push_runs_no_ext_remote_a_run_set() {
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);
    let planted = user.path("planted/ext");
    script(
        &planted,
        &format!("echo ran >> '{}'\nexit 1\n", user.path("ext-ran").display()),
    );
    // As a run could write them in the repository's settings.
    let folder = user.path("work/app");
    user.git(&folder, &["config", "protocol.ext.allow", "always"]);
    user.git(
        &folder,
        &[
            "config",
            "remote.origin.pushurl",
            &format!("ext::{}", planted.display()),
        ],
    );
    let mcp = agent(&user);

    let pushed = mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/ext"}),
    );

    assert!(
        !user.path("ext-ran").exists(),
        "the ext remote ran: {}",
        pushed.text
    );
    assert!(!remote_has(&user, "ext"));
}

/// [`user`], whose daemon's session bus listens at `bus-outside/dbus-test`, outside the runtime
/// folders (as `dbus-launch` makes one): the listening bus.
fn user_with_bus_outside() -> (User, UnixListener, UnixListener) {
    let (user, agent, _bus) = user();
    let at = user.path("bus-outside/dbus-test");
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    let bus = UnixListener::bind(&at).unwrap();
    let user = user.with_env(
        "DBUS_SESSION_BUS_ADDRESS",
        &format!("unix:path={},guid=0123456789abcdef", at.display()),
    );
    (user, agent, bus)
}

/// Whether a confined run of `mcp` starts and ends well.
fn confined_runs(mcp: &Mcp) -> Result<(), String> {
    let answer = mcp.call("run", json!({"line": "git status --short"}));
    match !answer.error && answer.text.contains("exited with code 0") {
        true => Ok(()),
        false => Err(answer.text),
    }
}

#[test]
fn a_folder_at_the_session_bus_s_path_holds_no_later_run() {
    if !common::user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return;
    }
    let (user, _agent, bus) = user_with_bus_outside();
    let mcp = session(&user, SSH_REMOTE);
    confined_runs(&mcp).unwrap();
    // As a run that reaches the bus's folder could do: the socket made a folder.
    let at = user.path("bus-outside/dbus-test");
    drop(bus);
    std::fs::remove_file(&at).unwrap();
    std::fs::create_dir(&at).unwrap();

    confined_runs(&mcp).unwrap();
    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/folder"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(
        ssh_said(&user).contains(&"dbus=".to_owned()),
        "{:?}",
        ssh_said(&user)
    );
}

#[test]
fn a_socket_made_at_the_session_bus_s_path_gets_no_client() {
    if !common::user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return;
    }
    let (user, _agent, bus) = user_with_bus_outside();
    let mcp = session(&user, SSH_REMOTE);
    confined_runs(&mcp).unwrap();
    // As a run that reaches the bus's folder could do: a socket of its own in the bus's place.
    let at = user.path("bus-outside/dbus-test");
    drop(bus);
    std::fs::remove_file(&at).unwrap();
    let fake = UnixListener::bind(&at).unwrap();
    fake.set_nonblocking(true).unwrap();

    confined_runs(&mcp).unwrap();
    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/fake"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(
        ssh_said(&user).contains(&"dbus=".to_owned()),
        "{:?}",
        ssh_said(&user)
    );
    assert_eq!(
        fake.accept().map(|_| ()).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "a client reached the socket made in the bus's place"
    );
}

/// The output of the Python `body` run by `mcp`'s agent, confined, from a file of the user's.
fn python(user: &User, mcp: &Mcp, body: &str) -> String {
    let probe = user.path("probe.py");
    std::fs::write(&probe, format!("import os, socket, sys\n{body}\n")).unwrap();
    done(mcp.call(
        "run",
        json!({"line": format!("python3 {}", probe.display())}),
    ))
}

/// What a confined run of `mcp` gets connecting to the socket at `path`: `connected`, or the
/// error's class.
fn run_connects(user: &User, mcp: &Mcp, path: &Path) -> String {
    let out = python(
        user,
        mcp,
        &format!(
            "s = socket.socket(socket.AF_UNIX)\n\
             try:\n\
             \x20   s.connect({path:?})\n\
             \x20   print('outcome=connected')\n\
             except OSError as e:\n\
             \x20   print('outcome=' + type(e).__name__)\n",
            path = path.to_str().unwrap()
        ),
    );
    out.lines()
        .find_map(|line| line.trim().strip_prefix("outcome="))
        .unwrap_or_else(|| panic!("{out}"))
        .to_owned()
}

#[test]
fn a_replaced_bus_is_still_masked() {
    if !common::user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return;
    }
    let (user, _agent, bus) = user_with_bus_outside();
    let mcp = session(&user, SSH_REMOTE);
    confined_runs(&mcp).unwrap();
    // Something else listens at the bus's path now: a bus started there again, or a socket an
    // unconfined process made.
    let at = user.path("bus-outside/dbus-test");
    drop(bus);
    std::fs::remove_file(&at).unwrap();
    let replaced = UnixListener::bind(&at).unwrap();
    replaced.set_nonblocking(true).unwrap();

    let outcome = run_connects(&user, &mcp, &at);
    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/replaced"}),
    ));

    assert_ne!(outcome, "connected", "a run reached the bus's path");
    assert_eq!(
        replaced.accept().map(|_| ()).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "a run reached the socket at the bus's path"
    );
    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(
        ssh_said(&user).contains(&"dbus=".to_owned()),
        "{:?}",
        ssh_said(&user)
    );
}

#[test]
fn a_bus_whose_folder_was_moved_is_masked_at_its_path() {
    if !common::user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return;
    }
    let (user, _agent, _bus) = user_with_bus_outside();
    let mcp = session(&user, SSH_REMOTE);
    let folder = user.path("bus-outside");
    let moved = user.path("bus-moved");
    // A run moves the bus's folder away: only the masked socket itself is a mount point.
    let renamed = python(
        &user,
        &mcp,
        &format!(
            "os.rename({folder:?}, {moved:?})\nprint('renamed')\n",
            folder = folder.to_str().unwrap(),
            moved = moved.to_str().unwrap()
        ),
    );
    assert!(renamed.contains("renamed"), "{renamed}");
    assert!(moved.join("dbus-test").exists());
    // A socket at the bus's path again, which the next runs must not reach either.
    std::fs::create_dir(&folder).unwrap();
    let at = folder.join("dbus-test");
    let again = UnixListener::bind(&at).unwrap();
    again.set_nonblocking(true).unwrap();

    let outcome = run_connects(&user, &mcp, &at);

    assert_ne!(outcome, "connected", "a run reached the bus's path");
    assert_eq!(
        again.accept().map(|_| ()).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "a run reached the socket at the bus's path"
    );
}

#[test]
fn doctor_says_the_bus_was_replaced() {
    let (user, _agent, bus) = user_with_bus_outside();
    let mcp = session(&user, SSH_REMOTE);
    confined_runs(&mcp).unwrap();
    let at = user.path("bus-outside/dbus-test");
    drop(bus);
    std::fs::remove_file(&at).unwrap();
    let _replaced = UnixListener::bind(&at).unwrap();

    let doctor = user.run(&["doctor", "--json"]);

    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert!(
        report["warnings"]
            .to_string()
            .contains("session bus the daemon found at its start"),
        "{report}"
    );
}

#[test]
fn an_allowed_push_takes_the_options_it_was_judged_with() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);

    let pushed = line(
        &user,
        &mcp,
        // No setting on the line: it would win over the daemon's own (refused, below).
        "git --no-pager push --recurse-submodules=check origin HEAD:refs/heads/optioned",
    );

    assert!(
        pushed.text.contains("exited with code 0"),
        "{}",
        pushed.text
    );
    assert!(remote_has(&user, "optioned"), "{}", pushed.text);
}

#[test]
fn an_allowed_push_runs_no_hook() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    let ran = user.path("hook-ran");
    let hook = format!("echo ran >> '{}'\nexit 1\n", ran.display());
    // The repository's own hook, and one of a folder its settings name: files an agent writes.
    script(&user.path("work/app/.git/hooks/pre-push"), &hook);
    script(&user.path("work/app/hooks/pre-push"), &hook);
    user.git(
        &user.path("work/app"),
        &["config", "core.hooksPath", "hooks"],
    );

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/allowed"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(remote_has(&user, "allowed"));
    assert!(!ran.exists(), "a hook ran");
}

#[test]
fn an_allowed_push_takes_no_project_variable() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    let ran = user.path("hook-ran");
    script(
        &user.path("hooks/pre-push"),
        &format!("echo ran >> '{}'\nexit 1\n", ran.display()),
    );
    // The project's variables are a file a run may write: one would undo the hooks setting.
    let file = user.project_file("app");
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(&format!(
        "\n[variables]\nGIT_CONFIG_PARAMETERS = \"'core.hookspath'='{hooks}'\"\n\
         FLOTTILLE_PLANTED = \"yes\"\n",
        hooks = user.path("hooks").display(),
    ));
    std::fs::write(&file, text).unwrap();
    let mcp = {
        drop(mcp);
        agent(&user)
    };

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/allowed"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(remote_has(&user, "allowed"));
    assert!(!ran.exists(), "the variables' hook ran");
    let said = ssh_said(&user);
    assert!(said.contains(&"planted=".to_owned()), "{said:?}");
    assert!(!said.contains(&"planted=yes".to_owned()), "{said:?}");
}

#[test]
fn a_run_that_is_not_a_push_reaches_no_credential() {
    let (user, _agent, _bus) = user();
    let user = user.with_env("GH_TOKEN", "gho_invented0token0for0tests");
    let mcp = session(&user, SSH_REMOTE);

    let token = done(mcp.call("run", json!({"line": "printenv GH_TOKEN"})));
    assert!(token.contains("exited with code 0"), "{token}");
    assert!(!token.contains("gho_invented0token0for0tests"), "{token}");
    let gh = done(mcp.call("run", json!({"line": "printenv GH_CONFIG_DIR"})));
    assert!(gh.contains("exited with code 0"), "{gh}");
    assert!(gh.contains("empty"), "{gh}");
    let agent = done(mcp.call("run", json!({"line": "printenv SSH_AUTH_SOCK"})));
    assert!(agent.contains("exited with code 1"), "{agent}");
    assert!(!agent.contains(socket(&user).to_str().unwrap()), "{agent}");

    // Connecting by the path is denied where Landlock governs it: from its ninth ABI.
    if landlock_abi() < 9 {
        eprintln!(
            "skipped the connect: this kernel's Landlock (ABI {}) cannot deny connecting to a \
             socket",
            landlock_abi()
        );
        return;
    }
    let line = format!(
        "python3 -c \"import socket; socket.socket(socket.AF_UNIX).connect('{}')\"",
        socket(&user).display()
    );
    let connected = done(mcp.call("run", json!({"line": line})));
    assert!(connected.contains("PermissionError"), "{connected}");
}

#[test]
fn a_wrapped_push_stays_in_the_sandbox() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    script(
        &user.path("work/app/push.sh"),
        "printenv SSH_AUTH_SOCK || echo no-agent\n\
         git push origin HEAD:refs/heads/scripted\n",
    );

    let scripted = line(&user, &mcp, "./push.sh").text;
    // Run and walled, or refused: never pushed.
    let wrapped = line(
        &user,
        &mcp,
        "env A=b git push origin HEAD:refs/heads/wrapped",
    )
    .text;

    assert!(scripted.contains("no-agent"), "{scripted}");
    assert!(!scripted.contains("exited with code 0"), "{scripted}");
    assert!(!wrapped.contains("exited with code 0"), "{wrapped}");
    assert!(!remote_has(&user, "scripted"));
    assert!(!remote_has(&user, "wrapped"));
    assert!(
        !ssh_said(&user)
            .iter()
            .any(|said| said.starts_with("reached")),
        "{:?}",
        ssh_said(&user)
    );
}

#[test]
fn an_agent_socket_outside_the_runtime_folder_is_reached_by_the_push_alone() {
    let (user, _agent, _bus) = user_with("agents/agent.sock");
    let mcp = session(&user, SSH_REMOTE);
    let socket = socket_at(&user, "agents/agent.sock");

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/allowed"}),
    ));
    let printed = done(mcp.call("run", json!({"line": "printenv SSH_AUTH_SOCK"})));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    let reached = format!("reached {}", socket.display());
    assert!(ssh_said(&user).contains(&reached), "{:?}", ssh_said(&user));
    assert!(printed.contains("exited with code 1"), "{printed}");
    if landlock_abi() < 9 {
        eprintln!(
            "skipped the connect: this kernel's Landlock (ABI {}) cannot deny connecting to a \
             socket",
            landlock_abi()
        );
        return;
    }
    let line = format!(
        "python3 -c \"import socket; socket.socket(socket.AF_UNIX).connect('{}')\"",
        socket.display()
    );
    let connected = done(mcp.call("run", json!({"line": line})));
    assert!(connected.contains("PermissionError"), "{connected}");
}

#[test]
fn a_push_tasks_ready_and_url_commands_get_no_ssh_agent() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    let probed = user.path("probed");
    script(
        &user.path("work/app/probe.sh"),
        &format!(
            "echo \"agent=$SSH_AUTH_SOCK\" >> '{}'\necho http://localhost:9/\n",
            probed.display()
        ),
    );
    user.call(
        "task.add",
        json!({"project": "app", "name": "publish",
            "line": "git push origin HEAD:refs/heads/tasked",
            "ready": {"command": "./probe.sh"}, "url": {"command": "./probe.sh"}}),
    )
    .unwrap();

    let answer = mcp.call("run", json!({"task": "publish"}));
    let answer = if answer.text.starts_with("Waiting for the user's approval") {
        allowed(&user, answer)
    } else {
        answer
    };

    let deadline = Instant::now() + Duration::from_secs(30);
    while !remote_has(&user, "tasked") {
        assert!(Instant::now() < deadline, "never pushed: {}", answer.text);
        std::thread::sleep(Duration::from_millis(50));
    }
    let said = std::fs::read_to_string(&probed).unwrap_or_default();
    assert!(said.contains("agent=\n"), "{said:?}");
    assert!(!said.contains(socket(&user).to_str().unwrap()), "{said:?}");
}

/// The socket lent to the daemon's own fetch: the one found at its start, links followed.
fn lent(user: &User) -> PathBuf {
    socket(user).canonicalize().unwrap()
}

/// The workspace `login` of `app`, made now: its first repository's base, as `workspace.create`
/// answers it.
fn created(user: &User) -> serde_json::Value {
    let created = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login"}),
        )
        .unwrap();
    created["repositories"][0]["base"].clone()
}

#[test]
fn a_planted_ssh_command_does_not_run_in_the_daemon_s_fetch() {
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);
    let folder = user.path("work/app");
    let planted = user.path("planted/ssh");
    script(
        &planted,
        &format!(
            "echo ran >> '{}'\nexit 1\n",
            user.path("planted-ran").display()
        ),
    );
    // Through an included file, and the remote's own upload command, as a run could write them.
    let included = user.path("planted/settings");
    std::fs::write(
        &included,
        format!("[core]\n\tsshCommand = {}\n", planted.display()),
    )
    .unwrap();
    user.git(
        &folder,
        &["config", "include.path", included.to_str().unwrap()],
    );
    user.git(
        &folder,
        &[
            "config",
            "remote.origin.uploadpack",
            planted.to_str().unwrap(),
        ],
    );

    let base = created(&user);

    assert!(!user.path("planted-ran").exists(), "the planted ssh ran");
    assert_eq!(base["fetched_now"], true, "{base}");
    // Git asked for its own upload command, not the remote's.
    let said = ssh_said(&user);
    let asked: Vec<_> = said.iter().filter(|s| s.starts_with("asked ")).collect();
    assert!(!asked.is_empty(), "{said:?}");
    assert!(
        asked
            .iter()
            .all(|s| s.starts_with("asked git-upload-pack '")),
        "{said:?}"
    );
    assert_eq!(base["ref"], "refs/remotes/origin/main", "{base}");
    // The `ssh` found at the daemon's start, lent the agent for this fetch alone.
    let reached = format!("reached {}", lent(&user).display());
    assert!(ssh_said(&user).contains(&reached), "{:?}", ssh_said(&user));
}

#[test]
fn the_daemon_s_git_cannot_reach_the_runtime_folder() {
    if landlock_abi() < 2 {
        eprintln!(
            "skipped: this kernel's Landlock (ABI {}) cannot keep the runtime folder out",
            landlock_abi()
        );
        return;
    }
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);

    let base = created(&user);

    assert_eq!(base["fetched_now"], true, "{base}");
    let said = ssh_said(&user);
    assert!(said.contains(&"runtime unlisted".to_owned()), "{said:?}");
    assert!(!said.contains(&"runtime listed".to_owned()), "{said:?}");
    // Connecting by a path is governed from the ninth ABI: the bus stays out, the agent lent.
    if landlock_abi() < 9 {
        eprintln!(
            "skipped the connect: this kernel's Landlock (ABI {}) cannot deny connecting to a \
             socket",
            landlock_abi()
        );
        return;
    }
    assert!(said.contains(&"bus refused".to_owned()), "{said:?}");
    assert!(!said.contains(&"bus reached".to_owned()), "{said:?}");
    let reached = format!("reached {}", lent(&user).display());
    assert!(said.contains(&reached), "{said:?}");
}

#[test]
fn an_agent_cli_starts_without_ssh_auth_sock() {
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);
    user.fake_script(json!({"turns": [{"when": "hello", "do": [
        {"say": "agent=<"}, {"say_variable": "SSH_AUTH_SOCK"}, {"say": ">"}
    ]}]}));

    let id = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let said = || user.call("agent.read", json!({"agent": id})).unwrap()["lines"].to_string();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !said().contains('>') {
        assert!(
            Instant::now() < deadline,
            "the agent never said: {}",
            said()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Empty: a value it had would show, masked.
    let said = said();
    assert!(said.contains("agent=<>"), "{said}");
}

#[test]
fn an_https_remote_is_still_fetched_by_the_daemon() {
    let user = credential_helper(User::with_fake_agent());
    std::fs::create_dir_all(user.path("remotes")).unwrap();
    let (_server, url) = http_remote(&user);
    project(&user, &url);

    let base = created(&user);

    assert_eq!(base["fetched_now"], true, "{base}");
    assert!(
        helper_said(&user).contains(&"get".to_owned()),
        "{:?}",
        helper_said(&user)
    );
}

#[test]
fn a_remote_rewritten_to_ssh_is_still_fetched_with_the_agent() {
    let (user, _agent, _bus) = user();
    // A path in the repository's settings, which git rewrites to the SSH remote.
    project(&user, "/nowhere/acme/app.git");
    let folder = user.path("work/app");
    user.git(
        &folder,
        &[
            "config",
            "url.git@forge.invalid:acme/.insteadOf",
            "/nowhere/acme/",
        ],
    );

    let base = created(&user);

    assert_eq!(base["fetched_now"], true, "{base}");
    let reached = format!("reached {}", lent(&user).display());
    assert!(ssh_said(&user).contains(&reached), "{:?}", ssh_said(&user));
}

#[test]
fn an_ext_remote_runs_nothing_in_the_daemon_s_fetch() {
    let (user, _agent, _bus) = user();
    let ran = user.path("helper-ran");
    // `ext::` runs the command its URL names, `% ` a space in it.
    project(
        &user,
        &format!("ext::sh -c echo% ext% >>% {}", ran.display()),
    );

    let base = created(&user);

    assert!(!ran.exists(), "the remote's command ran");
    assert_eq!(base["fetched_now"], false, "{base}");
}

#[test]
fn a_remote_helper_of_the_path_runs_nothing_in_the_daemon_s_fetch() {
    let (user, _agent, _bus) = user();
    let ran = user.path("helper-ran");
    // `foo::` and `foo://` run `git-remote-foo`, found on the daemon's `PATH`.
    script(
        &user.path("ssh-bin/git-remote-foo"),
        &format!("echo foo >> '{}'\nexit 1\n", ran.display()),
    );
    project(&user, "foo::forge.invalid/acme/app");

    let base = created(&user);

    assert!(!ran.exists(), "git-remote-foo ran");
    assert_eq!(base["fetched_now"], false, "{base}");
}

#[test]
fn a_repository_s_credential_helper_never_sees_the_user_s_sign_in() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    let folder = user.path("work/app");
    let said = user.path("planted-said");
    let planted = user.path("planted/helper");
    script(&planted, &format!("echo \"$1\" >> '{}'\n", said.display()));
    // The repository's own helpers, for every URL and for this one: files a run may write.
    let helper = format!("!{}", planted.display());
    user.git(&folder, &["config", "credential.helper", &helper]);
    user.git(
        &folder,
        &["config", &format!("credential.{url}.helper"), &helper],
    );

    let base = created(&user);

    assert_eq!(base["fetched_now"], true, "{base}");
    assert!(
        helper_said(&user).contains(&"get".to_owned()),
        "{:?}",
        helper_said(&user)
    );
    let planted_said = std::fs::read_to_string(&said).unwrap_or_default();
    assert_eq!(planted_said, "", "the repository's helper was asked");
}

#[test]
fn a_partial_clone_fetches_nothing_lazily_in_the_daemon_s_git() {
    let (user, _agent, _bus) = user();
    let ran = user.path("helper-ran");
    script(
        &user.path("ssh-bin/git-remote-foo"),
        &format!("echo foo >> '{}'\nexit 1\n", ran.display()),
    );
    let remote = user.path("remotes/app.git");
    let seed = user.path("seed");
    user.git_init(&seed);
    std::fs::write(seed.join("README.md"), "app\n").unwrap();
    user.git(&seed, &["add", "README.md"]);
    user.git(&seed, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(&remote, &["config", "uploadpack.allowFilter", "true"]);
    user.git(
        &seed,
        &["push", "--quiet", remote.to_str().unwrap(), "main"],
    );
    let folder = user.path("work/app");
    user.git(
        &user.path(""),
        &[
            "clone",
            "--quiet",
            "--filter=blob:none",
            &format!("file://{}", remote.display()),
            folder.to_str().unwrap(),
        ],
    );
    // A commit whose file the clone has not got: its promisor remote would be asked for it.
    std::fs::write(seed.join("later.txt"), "later\n").unwrap();
    user.git(&seed, &["add", "later.txt"]);
    user.git(&seed, &["commit", "--quiet", "-m", "later"]);
    user.git(
        &seed,
        &["push", "--quiet", remote.to_str().unwrap(), "main"],
    );
    user.git(&folder, &["fetch", "--quiet", "origin"]);
    // The promisor remote, as a run could rewrite it: a helper of the `PATH`.
    user.git(
        &folder,
        &["remote", "set-url", "origin", "foo::forge.invalid/acme/app"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();

    let _ = user.call(
        "workspace.create",
        json!({"project": "app", "name": "login"}),
    );

    assert!(!ran.exists(), "the promisor remote's helper ran");
}

#[test]
fn a_planted_git_proxy_does_not_run_in_the_daemon_s_fetch() {
    let (user, _agent, _bus) = user();
    project(&user, "git://forge.invalid/acme/app.git");
    let planted = user.path("planted/proxy");
    script(
        &planted,
        &format!(
            "echo ran >> '{}'\nexit 1\n",
            user.path("planted-ran").display()
        ),
    );
    user.git(
        &user.path("work/app"),
        &["config", "core.gitProxy", planted.to_str().unwrap()],
    );

    let base = created(&user);

    assert!(!user.path("planted-ran").exists(), "the planted proxy ran");
    assert_eq!(base["fetched_now"], false, "{base}");
}

#[test]
fn a_planted_askpass_does_not_run_in_the_daemon_s_fetch() {
    let user = User::with_fake_agent();
    std::fs::create_dir_all(user.path("remotes")).unwrap();
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    let planted = user.path("planted/askpass");
    script(
        &planted,
        &format!(
            "echo ran >> '{}'\necho invented\n",
            user.path("planted-ran").display()
        ),
    );
    user.git(
        &user.path("work/app"),
        &["config", "core.askPass", planted.to_str().unwrap()],
    );

    let base = created(&user);

    assert!(
        !user.path("planted-ran").exists(),
        "the planted askpass ran"
    );
    assert_eq!(base["fetched_now"], false, "{base}");
}

#[test]
fn the_ssh_found_at_the_start_is_its_target_not_its_link() {
    let (user, _agent, _bus) = user();
    // The fake `ssh` behind a link of the `PATH`, which is pointed elsewhere once the daemon runs.
    let fake = user.path("ssh-real/ssh");
    std::fs::create_dir_all(fake.parent().unwrap()).unwrap();
    std::fs::rename(user.path("ssh-bin/ssh"), &fake).unwrap();
    std::os::unix::fs::symlink(&fake, user.path("ssh-bin/ssh")).unwrap();
    project(&user, SSH_REMOTE);
    let planted = user.path("planted/ssh");
    script(
        &planted,
        &format!(
            "echo ran >> '{}'\nexit 1\n",
            user.path("planted-ran").display()
        ),
    );
    std::fs::remove_file(user.path("ssh-bin/ssh")).unwrap();
    std::os::unix::fs::symlink(&planted, user.path("ssh-bin/ssh")).unwrap();

    let base = created(&user);

    assert!(
        !user.path("planted-ran").exists(),
        "the link's new target ran"
    );
    assert_eq!(base["fetched_now"], true, "{base}");
    let reached = format!("reached {}", lent(&user).display());
    assert!(ssh_said(&user).contains(&reached), "{:?}", ssh_said(&user));
}

/// The packs of the repository `folder`.
fn packs(folder: &Path) -> usize {
    std::fs::read_dir(folder.join(".git/objects/pack"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "pack"))
        .count()
}

#[test]
fn the_daemon_s_fetch_starts_no_maintenance() {
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);
    let folder = user.path("work/app");
    // A commit the remote has and the repository has not, fetched as a pack of its own.
    let seed = user.path("seed");
    let remote = user.path("remotes/app.git");
    user.git(
        &user.path(""),
        &[
            "clone",
            "--quiet",
            "--branch",
            "main",
            remote.to_str().unwrap(),
            seed.to_str().unwrap(),
        ],
    );
    std::fs::write(seed.join("later.txt"), "later\n").unwrap();
    user.git(&seed, &["add", "later.txt"]);
    user.git(&seed, &["commit", "--quiet", "-m", "later"]);
    user.git(&seed, &["push", "--quiet", "origin", "main"]);
    // Settings that make the maintenance after a fetch pack it all at once, in the foreground.
    for (name, value) in [
        ("fetch.unpackLimit", "1"),
        ("gc.autoPackLimit", "1"),
        ("gc.autoDetach", "false"),
        ("maintenance.autoDetach", "false"),
    ] {
        user.git(&folder, &["config", name, value]);
    }
    user.git(&folder, &["repack", "--quiet"]);
    assert_eq!(packs(&folder), 1);

    let base = created(&user);

    assert_eq!(base["fetched_now"], true, "{base}");
    assert_eq!(packs(&folder), 2, "the fetch's pack was packed again");
}

#[test]
fn an_https_fetch_of_the_daemon_gets_no_ssh_agent() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    project(&user, &url);

    let base = created(&user);

    assert_eq!(base["fetched_now"], true, "{base}");
    let said = helper_said(&user);
    assert!(said.contains(&"agent=".to_owned()), "{said:?}");
    assert!(
        !said
            .iter()
            .any(|s| s.starts_with("agent=") && s != "agent="),
        "{said:?}"
    );
}

#[test]
fn a_file_fetch_of_the_daemon_gets_no_ssh_agent() {
    let (user, _agent, _bus) = user();
    // A program of the user's own settings that the remote's side of a fetch runs.
    let said = user.path("hook-said");
    let hook = user.path("hooks/pack-objects");
    script(
        &hook,
        &format!(
            "echo \"agent=$SSH_AUTH_SOCK\" >> '{}'\nexec \"$@\"\n",
            said.display()
        ),
    );
    std::fs::create_dir_all(user.path("home")).unwrap();
    let settings = user.path("home/.gitconfig");
    std::fs::write(
        &settings,
        format!("[uploadpack]\n\tpackObjectsHook = {}\n", hook.display()),
    )
    .unwrap();
    let user = user.with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap());
    let remote = user.path("remotes/app.git");
    project(&user, remote.to_str().unwrap());
    // A commit to fetch.
    let seed = user.path("seed");
    user.git(
        &user.path(""),
        &[
            "clone",
            "--quiet",
            "--branch",
            "main",
            remote.to_str().unwrap(),
            seed.to_str().unwrap(),
        ],
    );
    std::fs::write(seed.join("later.txt"), "later\n").unwrap();
    user.git(&seed, &["add", "later.txt"]);
    user.git(&seed, &["commit", "--quiet", "-m", "later"]);
    user.git(&seed, &["push", "--quiet", "origin", "main"]);

    let base = created(&user);

    assert_eq!(base["fetched_now"], true, "{base}");
    let said = std::fs::read_to_string(&said).unwrap_or_default();
    assert!(said.contains("agent=\n"), "{said:?}");
    assert!(!said.contains("agent=/"), "{said:?}");
}

#[test]
fn an_agent_socket_outside_the_runtime_folder_is_lent_to_the_daemon_s_fetch() {
    let (user, _agent, _bus) = user_with("agents/agent.sock");
    project(&user, SSH_REMOTE);

    let base = created(&user);

    assert_eq!(base["fetched_now"], true, "{base}");
    let lent = socket_at(&user, "agents/agent.sock")
        .canonicalize()
        .unwrap();
    let reached = format!("reached {}", lent.display());
    assert!(ssh_said(&user).contains(&reached), "{:?}", ssh_said(&user));
}

/// A program at `planted/<name>` that notes in `<name>-ran` that it ran, then fails: what a run
/// could name in the repository's settings.
fn planted(user: &User, name: &str) -> PathBuf {
    let planted = user.path(&format!("planted/{name}"));
    script(
        &planted,
        &format!(
            "echo ran >> '{}'\nexit 1\n",
            user.path(&format!("{name}-ran")).display()
        ),
    );
    planted
}

#[test]
fn a_receivepack_a_run_wrote_never_runs_on_push() {
    let (user, _agent, _bus) = user();
    let remote = user.path("remotes/app.git");
    project(&user, remote.to_str().unwrap());
    let planted = planted(&user, "receivepack");
    // As a run could write it (`git config remote.origin.receivepack …`): a remote reached by
    // its path runs it on this machine.
    user.git(
        &user.path("work/app"),
        &[
            "config",
            "remote.origin.receivepack",
            planted.to_str().unwrap(),
        ],
    );
    // A branch format: the checks' dry run reaches the remote first.
    user.call(
        "project.set_conventions",
        json!({"name": "app", "branch_format": "^(feat|fix)/[a-z-]+$"}),
    )
    .unwrap();
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/fix/received");

    assert!(
        !user.path("receivepack-ran").exists(),
        "the planted receivepack ran: {}",
        pushed.text
    );
    assert!(
        pushed.text.contains("exited with code 0"),
        "{}",
        pushed.text
    );
    assert!(remote_has(&user, "fix/received"));
}

#[test]
fn a_git_proxy_a_run_wrote_never_runs_on_push() {
    let (user, _agent, _bus) = user();
    project(&user, "git://forge.invalid/app.git");
    let planted = planted(&user, "proxy");
    user.git(
        &user.path("work/app"),
        &["config", "core.gitProxy", planted.to_str().unwrap()],
    );
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/proxied");

    assert!(
        !user.path("proxy-ran").exists(),
        "the planted proxy ran: {}",
        pushed.text
    );
}

#[test]
fn an_askpass_a_run_wrote_never_runs_on_push() {
    let (user, _agent, _bus) = user();
    // No credential helper: git asks for the sign-in.
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    let planted = planted(&user, "askpass");
    user.git(
        &user.path("work/app"),
        &["config", "core.askPass", planted.to_str().unwrap()],
    );
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/asked");

    assert!(
        !user.path("askpass-ran").exists(),
        "the planted askpass ran: {}",
        pushed.text
    );
}

#[test]
fn the_ssh_command_of_an_include_for_the_repository_is_used() {
    let (user, _agent, _bus) = user();
    // The user's command for the accounts of `~/work`, in a file their settings include for the
    // repositories there.
    let own = user.path("home/.ssh/work-ssh");
    marker(&user, &own);
    std::fs::write(
        user.path("home/.gitconfig-work"),
        format!("[core]\n\tsshCommand = {}\n", own.display()),
    )
    .unwrap();
    let global = user.path("home/.gitconfig");
    std::fs::write(
        &global,
        format!(
            "[includeIf \"gitdir:{}/\"]\n\tpath = {}\n",
            user.path("work").display(),
            user.path("home/.gitconfig-work").display()
        ),
    )
    .unwrap();
    let user = user.with_env("GIT_CONFIG_GLOBAL", global.to_str().unwrap());
    let mcp = session(&user, SSH_REMOTE);

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/work"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(
        user.path("home/.ssh/work-ssh-ran").exists(),
        "the user's ssh for the repository did not run"
    );
    assert!(remote_has(&user, "work"));
}

#[test]
fn a_push_lent_to_nothing_runs_no_ssh() {
    let (user, _agent, _bus) = user();
    // A configuration that reaches a file a run can write.
    std::fs::create_dir_all(user.path("home/.ssh")).unwrap();
    std::fs::write(user.path("home/.ssh/config"), "Include ~/dots/forge.conf\n").unwrap();
    std::fs::create_dir_all(user.path("home/dots")).unwrap();
    std::fs::write(user.path("home/dots/forge.conf"), "Host forge.invalid\n").unwrap();
    let mcp = session(&user, SSH_REMOTE);

    let pushed = mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/unlent"}),
    );

    // No `ssh` ran for the push (the daemon's own reads may have run the one it found).
    assert!(
        !ssh_said(&user)
            .iter()
            .any(|said| said.contains("git-receive-pack")),
        "{:?}",
        ssh_said(&user)
    );
    assert!(!remote_has(&user, "unlent"));
    assert!(pushed.text.contains("lent to no push"), "{}", pushed.text);
    assert!(pushed.text.contains("dots/forge.conf"), "{}", pushed.text);
}

#[test]
fn a_user_ssh_command_a_run_can_change_lends_no_push() {
    let (user, _agent, _bus) = user();
    // A program outside `~/.ssh`, in a folder a run writes.
    let own = user.path("own/ssh");
    marker(&user, &own);
    let user = user.with_env("GIT_SSH_COMMAND", own.to_str().unwrap());
    let mcp = session(&user, SSH_REMOTE);

    let pushed = mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/changed"}),
    );

    assert!(!user.path("own/ssh-ran").exists(), "{}", pushed.text);
    assert!(!remote_has(&user, "changed"));
    assert!(pushed.text.contains("lent to no push"), "{}", pushed.text);
}

/// A hook at `path` that notes in `hook-said` whether it could reach the user's SSH agent and
/// session bus by their paths, as a hook a run wrote would try.
fn noting_hook(user: &User, path: &Path) {
    let connect = "python3 -c 'import socket, sys; \
                   socket.socket(socket.AF_UNIX).connect(sys.argv[1])'";
    script(
        path,
        &format!(
            "if {connect} '{agent}' 2>/dev/null; then echo 'agent reached' >> '{said}'\n\
             else echo 'agent refused' >> '{said}'; fi\n\
             if {connect} '{runtime}/bus' 2>/dev/null; then echo 'bus reached' >> '{said}'\n\
             else echo 'bus refused' >> '{said}'; fi\n",
            agent = socket(user).display(),
            runtime = user.path("runtime").display(),
            said = user.path("hook-said").display(),
        ),
    );
}

fn hook_said(user: &User) -> Vec<String> {
    std::fs::read_to_string(user.path("hook-said"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Whether the bare repository at `bare` has the branch.
fn has(bare: &Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .arg("--git-dir")
        .arg(bare)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn a_push_to_a_path_runs_the_target_s_hooks_in_the_sandbox() {
    if landlock_abi() < 2 {
        eprintln!("skipped: no Landlock to keep the agent out of a confined command's reach");
        return;
    }
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);
    // A bare repository a run made, with a hook it wrote, and the remote's push URL it set.
    let target = user.path("planted.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", target.to_str().unwrap()],
    );
    noting_hook(&user, &target.join("hooks/post-receive"));
    user.git(
        &user.path("work/app"),
        &["config", "remote.origin.pushurl", target.to_str().unwrap()],
    );
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/planted");

    let said = hook_said(&user);
    assert!(!said.contains(&"agent reached".to_owned()), "{said:?}");
    assert!(!said.contains(&"bus reached".to_owned()), "{said:?}");
    // A push to a path needs no credential of the user's: it goes, as a run's own would.
    assert!(has(&target, "planted"), "{}", pushed.text);
}

#[test]
fn a_push_runs_no_submodule_push_a_run_set_up() {
    if landlock_abi() < 2 {
        eprintln!("skipped: no Landlock to keep the agent out of a confined command's reach");
        return;
    }
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);
    // A submodule whose remote is a bare repository of the run's, with a commit it lacks.
    let target = user.path("sub.git");
    let sub = user.path("sub");
    user.git_init(&sub);
    std::fs::write(sub.join("a.txt"), "a\n").unwrap();
    user.git(&sub, &["add", "a.txt"]);
    user.git(&sub, &["commit", "--quiet", "-m", "a"]);
    // Its `main` the branch a clone checks out.
    user.git(
        &user.path(""),
        &[
            "init",
            "--quiet",
            "--bare",
            "--initial-branch=main",
            target.to_str().unwrap(),
        ],
    );
    user.git(
        &sub,
        &[
            "push",
            "--quiet",
            target.to_str().unwrap(),
            "HEAD:refs/heads/main",
        ],
    );
    let folder = user.path("work/app");
    user.git(
        &folder,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "--quiet",
            "add",
            target.to_str().unwrap(),
            "sub",
        ],
    );
    let inner = folder.join("sub");
    std::fs::write(inner.join("b.txt"), "b\n").unwrap();
    user.git(&inner, &["add", "b.txt"]);
    user.git(&inner, &["commit", "--quiet", "-m", "b"]);
    user.git(&folder, &["add", "sub", ".gitmodules"]);
    user.git(&folder, &["commit", "--quiet", "-m", "sub"]);
    noting_hook(&user, &target.join("hooks/post-receive"));
    // As a run could write it: the submodule's commits pushed with the repository's.
    user.git(&folder, &["config", "push.recurseSubmodules", "on-demand"]);
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/with-sub");

    let said = hook_said(&user);
    assert!(!said.contains(&"agent reached".to_owned()), "{said:?}");
    assert!(!said.contains(&"bus reached".to_owned()), "{said:?}");
    assert!(remote_has(&user, "with-sub"), "{}", pushed.text);
}

#[test]
fn an_allowed_push_runs_no_credential_helper_a_run_wrote() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    let planted = user.path("planted/helper");
    script(
        &planted,
        &format!(
            "echo \"$1\" >> '{}'\n",
            user.path("planted-helper-ran").display()
        ),
    );
    // As a run could write it in the repository's settings.
    user.git(
        &user.path("work/app"),
        &["config", "credential.helper", planted.to_str().unwrap()],
    );
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/helped");

    assert!(
        !user.path("planted-helper-ran").exists(),
        "the planted helper ran: {}",
        pushed.text
    );
    assert!(remote_has(&user, "helped"), "{}", pushed.text);
    assert!(
        helper_said(&user).contains(&"get".to_owned()),
        "{:?}",
        helper_said(&user)
    );
}

#[test]
fn an_allowed_push_runs_no_git_a_run_put_first_in_the_path() {
    let (user, _agent, _bus) = user();
    // A folder of the user's first in the daemon's `PATH`, as `~/.local/bin` often is.
    let bin = user.path("home/.local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    // Before the fake `ssh` and the other programs `fake_ssh` put in it.
    let path = format!(
        "{}:{}:{}",
        bin.display(),
        user.path("ssh-bin").display(),
        user.path("programs").display()
    );
    let user = user.with_env("PATH", &path);
    let mcp = session(&user, SSH_REMOTE);
    // Written by a run once the daemon runs.
    script(
        &bin.join("git"),
        &format!(
            "case \"$*\" in *push*) echo ran >> '{ran}';; esac\nexec '{real}' \"$@\"\n",
            ran = user.path("planted-git-ran").display(),
            real = common::program("git").display(),
        ),
    );

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/real-git");

    assert!(
        !user.path("planted-git-ran").exists(),
        "the planted git ran: {}",
        pushed.text
    );
    assert!(remote_has(&user, "real-git"), "{}", pushed.text);
}

#[test]
fn an_allowed_push_s_ssh_shares_no_connection_and_forwards_no_agent() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/plain");

    assert!(remote_has(&user, "plain"), "{}", pushed.text);
    let said = ssh_said(&user);
    let args = said
        .iter()
        .find(|line| line.starts_with("args "))
        .unwrap_or_else(|| panic!("{said:?}"));
    // First, so that neither `~/.ssh/config` nor a later option of the line sets them.
    assert!(
        args.starts_with("args -o ControlMaster=no -o ControlPath=none -o ForwardAgent=no "),
        "{args}"
    );
}

/// A proxy of a run's on loopback: it adds the head of each request it is sent to `proxy-said`,
/// and answers 401 (as the remote would), so that git asks for a sign-in and sends it there. Its
/// URL.
fn run_s_proxy(user: &User) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let said = user.path("proxy-said");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut head = vec![0; 8192];
            let read = stream.read(&mut head).unwrap_or(0);
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&said)
                .unwrap();
            file.write_all(&head[..read]).unwrap();
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"forge\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[test]
fn a_proxy_a_run_set_never_sees_the_https_sign_in() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    let proxy = run_s_proxy(&user);
    // As a run could write them in the repository's settings: its proxy, for this remote alone
    // too (which a setting of the command's scope does not win over), and no check of the
    // certificate it would show.
    let folder = user.path("work/app");
    user.git(&folder, &["config", "http.proxy", &proxy]);
    user.git(&folder, &["config", &format!("http.{url}.proxy"), &proxy]);
    user.git(&folder, &["config", "http.sslVerify", "false"]);
    user.git(&folder, &["config", "http.extraHeader", "X-Planted: yes"]);
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/unproxied");

    let said = std::fs::read_to_string(user.path("proxy-said")).unwrap_or_default();
    assert!(
        said.is_empty(),
        "the run's proxy was sent: {said}\n{}",
        pushed.text
    );
    assert!(remote_has(&user, "unproxied"), "{}", pushed.text);
}

#[test]
fn a_url_rewrite_a_run_set_does_not_redirect_a_lifted_push() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    let elsewhere = user.path("remotes/elsewhere.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", elsewhere.to_str().unwrap()],
    );
    // As a run could write them in the repository's settings once the daemon runs.
    let folder = user.path("work/app");
    for setting in [
        "url.git@forge.invalid:acme/elsewhere.git.pushInsteadOf",
        "url.git@forge.invalid:acme/elsewhere.git.insteadOf",
    ] {
        user.git(&folder, &["config", setting, SSH_REMOTE]);
    }

    let pushed = line(
        &user,
        &mcp,
        "git push origin HEAD:refs/heads/not-redirected",
    );

    let redirected = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&elsewhere)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/not-redirected",
        ])
        .output()
        .unwrap()
        .status
        .success();
    assert!(!redirected, "the push went elsewhere: {}", pushed.text);
    assert!(remote_has(&user, "not-redirected"), "{}", pushed.text);
}

#[test]
fn a_url_rewrite_of_the_user_s_own_settings_for_the_folder_still_applies() {
    let (user, _agent, _bus) = user();
    let mine = user.path("remotes/mine.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", mine.to_str().unwrap()],
    );
    // The user's own settings, for the repositories of one folder: an `includeIf "gitdir:…"`.
    std::fs::create_dir_all(user.path("home")).unwrap();
    let included = user.path("home/work.gitconfig");
    std::fs::write(
        &included,
        format!("[url \"git@forge.invalid:acme/mine.git\"]\n\tpushInsteadOf = {SSH_REMOTE}\n"),
    )
    .unwrap();
    let settings = user.path("home/.gitconfig");
    std::fs::write(
        &settings,
        format!(
            "[includeIf \"gitdir:{}/\"]\n\tpath = {}\n",
            user.path("work").display(),
            included.display()
        ),
    )
    .unwrap();
    let user = user.with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap());
    let mcp = session(&user, SSH_REMOTE);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/mine");

    let landed = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&mine)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/mine"])
        .output()
        .unwrap()
        .status
        .success();
    assert!(landed, "{}", pushed.text);
}

#[test]
fn a_lifted_push_that_sets_the_upstream_sets_it_in_the_repository() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);

    let pushed = line(&user, &mcp, "git push -u origin HEAD:refs/heads/tracked");

    assert!(remote_has(&user, "tracked"), "{}", pushed.text);
    let merge = std::process::Command::new("git")
        .arg("-C")
        .arg(user.path("work/app"))
        .args(["config", "--get", "branch.main.merge"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&merge.stdout).trim(),
        "refs/heads/tracked",
        "{}",
        pushed.text
    );
}

#[test]
fn a_credential_helper_named_by_a_bare_name_runs_the_root_owned_program() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    // The user's helper named through `sh`, found by its bare name in the `PATH`.
    let settings = user.path("home/.gitconfig");
    std::fs::write(
        &settings,
        format!(
            "[credential]\n\thelper = \"!sh '{}'\"\n",
            user.path("helper/sign-in").display()
        ),
    )
    .unwrap();
    // A folder of the user's first in the daemon's `PATH`, as `~/.local/bin` often is, and the
    // system's folders after the test's own.
    let bin = user.path("home/.local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = format!(
        "{}:{}:{}:/usr/bin:/bin",
        bin.display(),
        user.path("ssh-bin").display(),
        user.path("programs").display()
    );
    let user = user.with_env("PATH", &path);
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    let mcp = agent(&user);
    // Written by a run once the daemon runs.
    script(
        &bin.join("sh"),
        &format!(
            "echo ran >> '{ran}'\nexec /bin/sh \"$@\"\n",
            ran = user.path("planted-sh-ran").display(),
        ),
    );

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/rooted");

    assert!(
        !user.path("planted-sh-ran").exists(),
        "the planted sh ran: {}",
        pushed.text
    );
    assert!(
        helper_said(&user).contains(&"get".to_owned()),
        "{:?} {}",
        helper_said(&user),
        pushed.text
    );
    assert!(remote_has(&user, "rooted"), "{}", pushed.text);
}

/// A message check of the project's, `checks-bin/msg-lint`, that refuses a message saying
/// `bad`: set before the agent's session starts, so that it reads it.
fn message_checked(user: &User) {
    script(
        &user.path("checks-bin/msg-lint"),
        "for last; do :; done\n\
         if grep -q bad \"$last\"; then echo 'msg: the subject says bad'; exit 1; fi\n",
    );
    let check = format!("{} {{message}}", user.path("checks-bin/msg-lint").display());
    user.call(
        "project.set_message_checks",
        json!({"name": "app", "checks": [check]}),
    )
    .unwrap();
}

/// A commit of the project's repository with this message.
fn committed(user: &User, message: &str) {
    let folder = user.path("work/app");
    std::fs::write(folder.join("README.md"), format!("{message}\n")).unwrap();
    user.git(&folder, &["commit", "--quiet", "-am", message]);
}

#[test]
fn a_proxy_a_run_set_never_sees_the_sign_in_of_a_checked_push() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    message_checked(&user);
    committed(&user, "a good change");
    let proxy = run_s_proxy(&user);
    let folder = user.path("work/app");
    user.git(&folder, &["config", &format!("http.{url}.proxy"), &proxy]);
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/checked");

    let said = std::fs::read_to_string(user.path("proxy-said")).unwrap_or_default();
    assert!(
        said.is_empty(),
        "the run's proxy was sent: {said}\n{}",
        pushed.text
    );
    assert!(remote_has(&user, "checked"), "{}", pushed.text);
    // The setting is one git follows: a read of the repository's own goes through the proxy.
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(&folder)
        .args(["ls-remote", "origin"])
        .env("GIT_CONFIG_GLOBAL", user.path("home/.gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    let said = std::fs::read_to_string(user.path("proxy-said")).unwrap_or_default();
    assert!(said.contains("Authorization: Basic"), "{said}");
}

#[test]
fn a_push_refspec_a_run_wrote_deletes_no_remote_branch() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    // As a run could write it: a plain `git push origin` would delete `main` on the remote.
    user.git(
        &user.path("work/app"),
        &["config", "remote.origin.push", ":refs/heads/main"],
    );

    let pushed = line(&user, &mcp, "git push origin");

    assert!(
        remote_has(&user, "main"),
        "main was deleted: {}",
        pushed.text
    );
    assert!(
        pushed.text.contains("remote.origin.push"),
        "{}",
        pushed.text
    );
}

#[test]
fn a_mirror_a_run_set_prunes_no_remote_branch() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    let folder = user.path("work/app");
    user.git(
        &folder,
        &[
            "--git-dir",
            user.path("remotes/app.git").to_str().unwrap(),
            "branch",
            "other",
            "main",
        ],
    );
    // As a run could write it: a plain `git push origin` would force every ref and prune the
    // remote's others.
    user.git(&folder, &["config", "remote.origin.mirror", "true"]);

    let pushed = line(&user, &mcp, "git push origin");

    assert!(
        remote_has(&user, "other"),
        "the remote was pruned: {}",
        pushed.text
    );
    assert!(
        pushed.text.contains("remote.origin.mirror"),
        "{}",
        pushed.text
    );
}

#[test]
fn a_push_default_a_run_wrote_sends_nothing_to_another_branch() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    let folder = user.path("work/app");
    committed(&user, "a change");
    // As a run could write them: a plain `git push` would send `main` to `elsewhere`.
    for (key, value) in [
        ("push.default", "upstream"),
        ("branch.main.remote", "origin"),
        ("branch.main.merge", "refs/heads/elsewhere"),
    ] {
        user.git(&folder, &["config", key, value]);
    }

    let pushed = line(&user, &mcp, "git push");

    assert!(
        !remote_has(&user, "elsewhere"),
        "the push went to another branch: {}",
        pushed.text
    );
    assert!(pushed.text.contains("push.default"), "{}", pushed.text);
}

#[test]
fn a_lifted_push_runs_no_program_of_an_exec_path_on_its_line() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    let tools = user.path("work/app/tools");
    // A run's own `git-remote-http`, which git takes from the exec path of the line.
    script(
        &tools.join("git-remote-http"),
        &format!(
            "echo ran >> '{}'\nexit 1\n",
            user.path("exec-path-ran").display()
        ),
    );
    let mcp = agent(&user);

    let pushed = line(
        &user,
        &mcp,
        &format!(
            "git --exec-path={} push origin HEAD:refs/heads/exec",
            tools.display()
        ),
    );

    assert!(
        !user.path("exec-path-ran").exists(),
        "the run's program ran: {}",
        pushed.text
    );
    assert!(
        pushed.text.contains("takes no --exec-path"),
        "{}",
        pushed.text
    );
    // The option is one git follows: its own read of the remote runs the run's program.
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(user.path("work/app"))
        .arg(format!("--exec-path={}", tools.display()))
        .args(["ls-remote", "origin"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(user.path("exec-path-ran").exists());
}

#[test]
fn a_setting_on_a_lifted_push_s_line_sends_no_sign_in_to_its_proxy() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    let proxy = run_s_proxy(&user);
    let mcp = agent(&user);

    let pushed = line(
        &user,
        &mcp,
        &format!("git -c http.proxy={proxy} push origin HEAD:refs/heads/lined"),
    );

    let said = std::fs::read_to_string(user.path("proxy-said")).unwrap_or_default();
    assert!(
        said.is_empty(),
        "the line's proxy was sent: {said}\n{}",
        pushed.text
    );
    assert!(pushed.text.contains("takes no setting"), "{}", pushed.text);
}

#[test]
fn grafts_a_run_wrote_hide_no_commit_from_the_message_checks() {
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);
    message_checked(&user);
    let folder = user.path("work/app");
    committed(&user, "a bad change");
    committed(&user, "a good change");
    // As a run could write them: the good change's parent is said to be the start, which the
    // remote has, so that the bad one is not read.
    let sha = |rev: &str| {
        user.git_output(&folder, &["rev-parse", rev])
            .trim()
            .to_owned()
    };
    let info = folder.join(".git/info");
    std::fs::create_dir_all(&info).unwrap();
    std::fs::write(
        info.join("grafts"),
        format!("{} {}\n", sha("HEAD"), sha("HEAD~2")),
    )
    .unwrap();
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/grafted");

    assert!(
        !remote_has(&user, "grafted"),
        "the bad change went unread: {}",
        pushed.text
    );
    assert!(pushed.text.contains("a bad change"), "{}", pushed.text);
}

#[test]
fn a_replaced_commit_hides_no_message_from_the_checks() {
    let (user, _agent, _bus) = user();
    project(&user, SSH_REMOTE);
    message_checked(&user);
    let folder = user.path("work/app");
    committed(&user, "a bad change");
    // As a run could write it: the bad change is read as a good one.
    let good = user
        .git_output(
            &folder,
            &[
                "commit-tree",
                "HEAD^{tree}",
                "-p",
                "HEAD~1",
                "-m",
                "a good change",
            ],
        )
        .trim()
        .to_owned();
    user.git(&folder, &["replace", "HEAD", &good]);
    let mcp = agent(&user);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/replaced");

    assert!(
        !remote_has(&user, "replaced"),
        "the bad change went unread: {}",
        pushed.text
    );
}

#[test]
fn a_push_is_not_lifted_where_no_landlock_keeps_its_folder_from_runs() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user).with_env("FLOTTILLE_TEST_NO_LANDLOCK", "1");
    let (_server, url) = http_remote(&user);
    let mcp = session(&user, &url);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/unprotected");

    assert!(!remote_has(&user, "unprotected"), "{}", pushed.text);
    assert!(helper_said(&user).is_empty(), "{:?}", helper_said(&user));
    assert!(pushed.text.contains("Landlock"), "{}", pushed.text);
}

#[test]
fn a_push_from_a_linked_worktree_goes() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    let folder = user.path("work/app");
    user.git(
        &folder,
        &["worktree", "add", "--quiet", "linked", "-b", "linked"],
    );
    let linked = folder.join("linked");
    std::fs::write(linked.join("README.md"), "linked\n").unwrap();
    user.git(
        &linked,
        &["commit", "--quiet", "-am", "from the linked worktree"],
    );

    let pushed = line(
        &user,
        &mcp,
        "git -C linked push origin HEAD:refs/heads/from-linked",
    );

    assert!(remote_has(&user, "from-linked"), "{}", pushed.text);
    // Its remote-tracking ref moved in the repository.
    let tracked = user.git_output(&folder, &["rev-parse", "refs/remotes/origin/from-linked"]);
    let head = user.git_output(&linked, &["rev-parse", "HEAD"]);
    assert_eq!(tracked, head, "{}", pushed.text);
}

#[test]
fn a_url_rewrite_of_the_user_s_settings_for_the_remote_s_forge_still_applies() {
    let (user, _agent, _bus) = user();
    let mine = user.path("remotes/mine.git");
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", mine.to_str().unwrap()],
    );
    // The user's own settings for the repositories of one forge: an
    // `includeIf "hasconfig:remote.*.url:…"`.
    std::fs::create_dir_all(user.path("home")).unwrap();
    let included = user.path("home/forge.gitconfig");
    std::fs::write(
        &included,
        format!("[url \"git@forge.invalid:acme/mine.git\"]\n\tpushInsteadOf = {SSH_REMOTE}\n"),
    )
    .unwrap();
    let settings = user.path("home/.gitconfig");
    std::fs::write(
        &settings,
        format!(
            "[includeIf \"hasconfig:remote.*.url:git@forge.invalid:acme/**\"]\n\tpath = {}\n",
            included.display()
        ),
    )
    .unwrap();
    let user = user.with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap());
    let mcp = session(&user, SSH_REMOTE);

    let pushed = line(&user, &mcp, "git push origin HEAD:refs/heads/forged");

    let landed = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&mine)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/forged"])
        .output()
        .unwrap()
        .status
        .success();
    assert!(landed, "{}", pushed.text);
}

#[test]
fn a_rewrite_for_reads_a_run_set_refuses_no_checked_push() {
    let (user, _agent, _bus) = user();
    let user = credential_helper(user);
    let (_server, url) = http_remote(&user);
    project(&user, &url);
    message_checked(&user);
    committed(&user, "a good change");
    // As a run could write it, for reads alone: the push does not follow it.
    let elsewhere = url.replace("app.git", "elsewhere.git");
    user.git(
        &user.path("work/app"),
        &["config", &format!("url.{elsewhere}.insteadOf"), &url],
    );
    let mcp = agent(&user);

    let pushed = line(
        &user,
        &mcp,
        "git push origin HEAD:refs/heads/read-rewritten",
    );

    assert!(remote_has(&user, "read-rewritten"), "{}", pushed.text);
}
