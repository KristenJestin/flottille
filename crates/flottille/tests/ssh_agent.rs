//! A push the gate let through runs outside the sandbox, as if the user had typed it: the user's
//! credential helper, SSH agent and settings, no hook and none of the project's variables
//! (`docs/commands.md`, "The environment", and `docs/vault.md`, "The runtime folder"). Every
//! other run reaches no credential. The agent is a listening socket of the test's runtime folder,
//! `ssh` a fake that says whether it could connect to it and serves git from a throwaway bare
//! repository, an HTTP remote a small server of the test's over `git http-backend` that asks for
//! a sign-in, and the agents and the judge are fake ACP agents.

mod common;

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
/// of `FLOTTILLE_PLANTED` it was given (`planted=…`), whether it could list the runtime folder
/// (`runtime listed`, `runtime unlisted`) and connect to its bus (`bus reached`, `bus refused`),
/// and serves git's own call from
/// `remotes/<name>`, whatever the host.
fn fake_ssh(user: User) -> User {
    std::fs::create_dir(user.path("ssh-bin")).unwrap();
    let said = user.path("ssh-said");
    let connect = "python3 -c 'import socket, sys; \
                   socket.socket(socket.AF_UNIX).connect(sys.argv[1])'";
    let script = format!(
        "#!/bin/sh\n\
         if [ -z \"$SSH_AUTH_SOCK\" ]; then echo 'no agent' >> '{said}'\n\
         elif {connect} \"$SSH_AUTH_SOCK\" 2>/dev/null; then echo \"reached $SSH_AUTH_SOCK\" >> '{said}'\n\
         else echo \"refused $SSH_AUTH_SOCK\" >> '{said}'; fi\n\
         echo \"planted=$FLOTTILLE_PLANTED\" >> '{said}'\n\
         if ls '{runtime}' >/dev/null 2>&1; then echo 'runtime listed' >> '{said}'\n\
         else echo 'runtime unlisted' >> '{said}'; fi\n\
         if {connect} '{runtime}/bus' 2>/dev/null; then echo 'bus reached' >> '{said}'\n\
         else echo 'bus refused' >> '{said}'; fi\n\
         for last; do :; done\n\
         service=${{last%% *}}\n\
         repository=$(basename \"$(echo \"${{last#* }}\" | tr -d \"'\")\")\n\
         exec git \"${{service#git-}}\" '{remotes}'/\"$repository\"\n",
        said = said.display(),
        remotes = user.path("remotes").display(),
        runtime = user.path("runtime").display(),
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
            "echo \"$1\" >> '{}'\n\
             if [ \"$1\" = get ]; then echo username=octo; echo password=invented-password; fi\n",
            user.path("helper-said").display()
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
fn an_allowed_push_takes_the_options_it_was_judged_with() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);

    let pushed = line(
        &user,
        &mcp,
        "git -c push.default=current push --recurse-submodules=check origin \
         HEAD:refs/heads/optioned",
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
