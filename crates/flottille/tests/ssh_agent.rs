//! The user's SSH agent lent to a push the gate let through, and to nothing else
//! (`docs/vault.md`, "The runtime folder", and `docs/tools.md`, "A push is the gate's"). The agent
//! is a listening socket of the test's runtime folder, `ssh` a fake that says whether it could
//! connect to it and serves git from a throwaway bare repository, and the agents and the judge are
//! fake ACP agents.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp, server_of};
use common::{User, landlock_abi};
use flottille_core::store::Store;
use serde_json::json;

/// A user whose daemon's `SSH_AUTH_SOCK` is `runtime/ssh-agent.sock` (listening), with a session
/// bus socket at `runtime/bus` (listening too) and the fake `ssh` of `fake_ssh` first in its
/// `PATH`.
fn user() -> (User, UnixListener, UnixListener) {
    user_with("runtime/ssh-agent.sock", "runtime/ssh-agent.sock")
}

/// `user`, the agent listening at `agent` and the daemon's `SSH_AUTH_SOCK` naming `named` (the
/// same path, or a link to it).
fn user_with(agent: &str, named: &str) -> (User, UnixListener, UnixListener) {
    let user = User::with_fake_agent().with_allowing_judge();
    let at = user.path(agent);
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    let agent = UnixListener::bind(&at).unwrap();
    let bus = UnixListener::bind(user.path("runtime/bus")).unwrap();
    let named = user.path(named);
    let user = fake_ssh(user).with_env("SSH_AUTH_SOCK", named.to_str().unwrap());
    (user, agent, bus)
}

/// The SSH agent's socket, as the daemon resolves it.
fn socket(user: &User) -> PathBuf {
    socket_at(user, "runtime/ssh-agent.sock")
}

fn socket_at(user: &User, agent: &str) -> PathBuf {
    user.path(agent).canonicalize().unwrap()
}

/// A fake `ssh` first in the user's `PATH`: each call adds to `ssh-said` whether it reached the
/// socket its `SSH_AUTH_SOCK` names (`reached <path>`, `refused <path>`, `no agent`), then whether
/// it reached the session bus (`bus reached`, `bus refused`) and could list the runtime folder
/// (`listed`, `not listed`), and serves git's own call from `remotes/<name>`, whatever the host.
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
         if {connect} '{bus}' 2>/dev/null; then echo 'bus reached' >> '{said}'\n\
         else echo 'bus refused' >> '{said}'; fi\n\
         if ls '{runtime}' >/dev/null 2>&1; then echo listed >> '{said}'\n\
         else echo 'not listed' >> '{said}'; fi\n\
         for last; do :; done\n\
         service=${{last%% *}}\n\
         repository=$(basename \"$(echo \"${{last#* }}\" | tr -d \"'\")\")\n\
         exec git \"${{service#git-}}\" '{remotes}'/\"$repository\"\n",
        said = said.display(),
        bus = user.path("runtime/bus").display(),
        runtime = user.path("runtime").display(),
        remotes = user.path("remotes").display(),
    );
    let ssh = user.path("ssh-bin/ssh");
    std::fs::write(&ssh, script).unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        user.path("ssh-bin").display(),
        std::env::var("PATH").unwrap_or_default()
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

#[test]
fn an_allowed_push_reaches_the_ssh_agent() {
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
fn a_run_that_is_not_a_push_gets_no_ssh_agent() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);

    let printed = done(mcp.call("run", json!({"line": "printenv SSH_AUTH_SOCK"})));
    assert!(printed.contains("exited with code 1"), "{printed}");
    assert!(
        !printed.contains(socket(&user).to_str().unwrap()),
        "{printed}"
    );

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
fn a_push_wrapped_in_another_command_gets_no_ssh_agent() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    let script = user.path("work/app/push.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintenv SSH_AUTH_SOCK || echo no-agent\n\
         git push origin HEAD:refs/heads/scripted\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let scripted = done(mcp.call("run", json!({"line": "./push.sh"})));
    // Run and walled, or refused: never pushed.
    let wrapped = mcp
        .call(
            "run",
            json!({"line": "env git push origin HEAD:refs/heads/wrapped"}),
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
fn a_push_run_reaches_nothing_else_of_the_runtime_folder() {
    if landlock_abi() < 2 {
        eprintln!(
            "skipped: this kernel's Landlock (ABI {}) is not used",
            landlock_abi()
        );
        return;
    }
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/allowed"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    let said = ssh_said(&user);
    assert!(said.contains(&"not listed".to_owned()), "{said:?}");
    assert!(!said.contains(&"listed".to_owned()), "{said:?}");
    if landlock_abi() < 9 {
        eprintln!(
            "skipped the bus: this kernel's Landlock (ABI {}) cannot deny connecting to a socket",
            landlock_abi()
        );
        return;
    }
    assert!(said.contains(&"bus refused".to_owned()), "{said:?}");
    assert!(!said.contains(&"bus reached".to_owned()), "{said:?}");
}

#[test]
fn an_https_remote_push_says_how_to_push() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, "https://forge.invalid/acme/app.git");

    let answer = mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/allowed"}),
    );

    assert!(answer.error, "{}", answer.text);
    assert!(
        answer.text.contains("git remote set-url --push"),
        "{}",
        answer.text
    );
    assert!(answer.text.contains("by hand"), "{}", answer.text);
    assert!(!remote_has(&user, "allowed"));
    assert_eq!(ssh_said(&user), Vec::<String>::new());
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

#[test]
fn a_swapped_agent_link_lends_nothing() {
    let (user, _agent, _bus) = user_with("runtime/ssh-agent.sock", "agent-link");
    std::os::unix::fs::symlink(user.path("runtime/ssh-agent.sock"), user.path("agent-link"))
        .unwrap();
    let mcp = session(&user, SSH_REMOTE);
    // The daemon has started: the link now leads to the session bus.
    std::fs::remove_file(user.path("agent-link")).unwrap();
    std::os::unix::fs::symlink(user.path("runtime/bus"), user.path("agent-link")).unwrap();

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/swapped"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    let said = ssh_said(&user);
    assert!(said.contains(&"no agent".to_owned()), "{said:?}");
    assert!(
        !said.iter().any(|said| said.starts_with("reached")),
        "{said:?}"
    );
}

#[test]
fn a_planted_ssh_command_does_not_run_on_a_push() {
    let (user, _agent, _bus) = user();
    let planted = user.path("planted");
    let plant = format!("echo ran >> '{}'\nexit 1\n", planted.display());
    // An `ssh` of the user's own `~/.local/bin`, which an agent could write, earlier on `PATH`.
    script(&user.path("home/.local/bin/ssh"), &plant);
    let path = format!(
        "{}:{}:{}",
        user.path("home/.local/bin").display(),
        user.path("ssh-bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let user = user.with_env("PATH", &path);
    let mcp = session(&user, SSH_REMOTE);
    // And the repository's own ssh command.
    script(&user.path("plant.sh"), &plant);
    user.git(
        &user.path("work/app"),
        &[
            "config",
            "core.sshCommand",
            user.path("plant.sh").to_str().unwrap(),
        ],
    );

    let pushed = done(mcp.call(
        "run",
        json!({"line": "git push origin HEAD:refs/heads/allowed"}),
    ));

    assert!(pushed.contains("exited with code 0"), "{pushed}");
    assert!(remote_has(&user, "allowed"));
    assert!(!planted.exists(), "a planted ssh ran");
    let reached = format!("reached {}", socket(&user).display());
    assert!(ssh_said(&user).contains(&reached), "{:?}", ssh_said(&user));
}

#[test]
fn a_push_with_c_is_refused() {
    let (user, _agent, _bus) = user();
    let mcp = session(&user, SSH_REMOTE);
    let hooked = user.path("hooked");
    script(
        &user.path("work/app/hooks/pre-push"),
        &format!("echo \"$SSH_AUTH_SOCK\" >> '{}'\n", hooked.display()),
    );

    for push in [
        "git -c core.hooksPath=hooks push origin HEAD:refs/heads/hooked",
        "git -ccore.hooksPath=hooks push origin HEAD:refs/heads/hooked",
        "git --config-env core.hooksPath=HOME push origin HEAD:refs/heads/hooked",
        "git --config-env=core.hooksPath=HOME push origin HEAD:refs/heads/hooked",
    ] {
        let answer = line(&user, &mcp, push);

        assert!(answer.error, "{push}: {}", answer.text);
        assert!(
            answer.text.contains("nothing was pushed"),
            "{push}: {}",
            answer.text
        );
        assert!(!remote_has(&user, "hooked"), "{push}");
        assert!(!hooked.exists(), "{push}: a hook ran");
    }
    assert!(
        !ssh_said(&user)
            .iter()
            .any(|said| said.starts_with("reached")),
        "{:?}",
        ssh_said(&user)
    );
}

#[test]
fn an_ext_url_is_refused_on_a_push() {
    let (user, _agent, _bus) = user();
    let ran = user.path("ext-ran");
    script(
        &user.path("ext.sh"),
        &format!("echo \"$SSH_AUTH_SOCK\" >> '{}'\nexit 1\n", ran.display()),
    );
    let url = format!("ext::{}", user.path("ext.sh").display());
    let mcp = session(&user, &url);
    user.git(
        &user.path("work/app"),
        &["config", "protocol.ext.allow", "always"],
    );

    let answer = line(&user, &mcp, "git push origin HEAD:refs/heads/ext");

    assert!(
        !answer.text.contains("exited with code 0"),
        "{}",
        answer.text
    );
    assert!(!ran.exists(), "the ext command ran");
}

#[test]
fn an_agent_socket_outside_the_runtime_folder_is_lent_to_the_push_alone() {
    let (user, _agent, _bus) = user_with("agents/agent.sock", "agents/agent.sock");
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

#[test]
fn a_push_whose_options_do_not_read_is_not_probed() {
    // A valued option left without its value takes the probe's `--dry-run` for it: the dry run
    // would be the push. The remote is a path, which the probe's own transport reaches.
    let (user, _agent, _bus) = user();
    let remote = user.path("remotes/app.git");
    let mcp = session(&user, remote.to_str().unwrap());
    // A remote that takes push options: otherwise the push fails whatever the probe did.
    user.git(&remote, &["config", "receive.advertisePushOptions", "true"]);

    let answer = line(&user, &mcp, "git push origin HEAD:refs/heads/optioned -o");

    assert!(
        !answer.text.contains("exited with code 0"),
        "{}",
        answer.text
    );
    assert!(!remote_has(&user, "optioned"), "{}", answer.text);
}
