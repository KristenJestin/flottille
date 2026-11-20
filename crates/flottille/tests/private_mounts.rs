//! The scenarios of the private mount namespace a run starts in (#976, `docs/vault.md`, "The
//! runtime folder"): the session bus out of reach by its path at any Landlock ABI, `~/.ssh` and
//! what it leads to read-only, and a run that still starts where the kernel refuses user
//! namespaces, the gap said. The agent is the fake ACP agent, in a throwaway repository, with
//! temporary XDG folders, home and runtime folder; the run is a small Python program.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp, server_of};
use common::{User, landlock_abi, user_namespaces};
use serde_json::{Value, json};

/// A project `app` over a throwaway repository, its workspace `w`, and the session of a root
/// agent working there in Full, so that nothing is held: its session, the workspace's folder.
fn session(user: &User) -> (Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let made = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while user
        .call("workspace.show", json!({"project": "app", "name": "w"}))
        .unwrap()["state"]
        != "ready"
    {
        assert!(Instant::now() < deadline, "the workspace never got ready");
        std::thread::sleep(Duration::from_millis(50));
    }
    let place = PathBuf::from(made["folder"].as_str().unwrap());
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "start the app"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap();
    let (url, authorization) = server_of(user, opened);
    let output = user.run(&["agent", "mode", id, "full"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (Mcp::initialize(&url, &authorization), place)
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

/// What the program `body` prints as its last line `{"out": …}`, run by the agent from the
/// workspace. `body` fills the dictionary `out`.
fn ran(mcp: &Mcp, place: &Path, body: &str) -> Value {
    std::fs::write(
        place.join("probe.py"),
        format!("import json, os, socket\nout = {{}}\n{body}\nprint(json.dumps({{'out': out}}))\n"),
    )
    .unwrap();
    let ran = done(mcp.call("run", json!({"line": "python3 probe.py"})));
    let line = ran
        .lines()
        .find(|line| line.trim_start().starts_with("{\"out\""))
        .unwrap_or_else(|| panic!("{ran}"));
    serde_json::from_str::<Value>(line.trim()).unwrap()["out"].clone()
}

/// The Python that tries to connect to the socket at `path`, its outcome under `out[name]`:
/// `connected`, or the error's class.
fn connect(name: &str, path: &str) -> String {
    format!(
        "s = socket.socket(socket.AF_UNIX)\n\
         try:\n\
         \x20   s.connect({path:?})\n\
         \x20   out[{name:?}] = 'connected'\n\
         except OSError as e:\n\
         \x20   out[{name:?}] = type(e).__name__\n"
    )
}

/// The Python that tries `action` (a statement), its outcome under `out[name]`: `done`, or the
/// error number.
fn attempt(name: &str, action: &str) -> String {
    format!(
        "try:\n\
         \x20   {action}\n\
         \x20   out[{name:?}] = 'done'\n\
         except OSError as e:\n\
         \x20   out[{name:?}] = e.errno\n"
    )
}

/// A path as Python builds it, `.ssh` never written whole: the gate refuses a program that names
/// it, and what is tested here is the mount, not the gate.
fn py_path(path: &Path) -> String {
    let text = path.to_str().unwrap();
    format!("{:?}", text).replace(".ssh", "\" + \".\" + \"ssh")
}

fn skipped_without_user_namespaces() -> bool {
    if !user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return true;
    }
    false
}

#[test]
fn a_run_cannot_connect_to_the_session_bus_by_its_path() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, place) = session(&user);
    let bus = user.path("runtime/bus");
    let _listening = std::os::unix::net::UnixListener::bind(&bus).unwrap();
    // A socket of the workspace: a run still connects to what is not in the runtime folder.
    let own = place.join("own.sock");
    let _own = std::os::unix::net::UnixListener::bind(&own).unwrap();

    let out = ran(
        &mcp,
        &place,
        &(connect("bus", bus.to_str().unwrap()) + &connect("own", own.to_str().unwrap())),
    );

    // Not there at all: the runtime folder is an empty one in the run's namespace, whatever
    // Landlock's ABI (where it rules connecting, the bus would be a PermissionError).
    assert_eq!(out["bus"], "FileNotFoundError", "{out}");
    assert_eq!(out["own"], "connected", "{out}");
}

#[test]
fn a_run_cannot_connect_to_the_session_bus_through_the_daemon_s_root() {
    if skipped_without_user_namespaces() {
        return;
    }
    // `/proc/<pid>/root` leads to another process's mounts: Landlock keeps a run from the
    // processes outside its domain.
    if landlock_abi() < 2 {
        eprintln!(
            "skipped: this kernel's Landlock (ABI {}) is not used",
            landlock_abi()
        );
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, place) = session(&user);
    let bus = user.path("runtime/bus");
    let _listening = std::os::unix::net::UnixListener::bind(&bus).unwrap();
    let pid = daemon_pid(&user);
    let through = format!("/proc/{pid}/root{}", bus.display());

    let out = ran(&mcp, &place, &connect("bus", &through));

    assert_ne!(out["bus"], "connected", "{out}");
}

/// The test daemon's process: Flottille's, with this user's state folder.
fn daemon_pid(user: &User) -> u32 {
    let program = std::fs::canonicalize(env!("CARGO_BIN_EXE_flottille")).unwrap();
    let state = format!("XDG_STATE_HOME={}", user.path("state").display());
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .find(|pid| {
            std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| exe == program)
                && std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environ| {
                    environ
                        .split(|byte| *byte == 0)
                        .any(|variable| variable == state.as_bytes())
                })
                && std::fs::read(format!("/proc/{pid}/cmdline"))
                    .is_ok_and(|line| line.split(|b| *b == 0).any(|word| word == b"daemon"))
        })
        .expect("the test daemon is running")
}

#[test]
fn a_run_starts_where_user_namespaces_are_refused() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_env("FLOTTILLE_TEST_NO_USER_NAMESPACES", "1");
    let (mcp, place) = session(&user);

    let out = ran(
        &mcp,
        &place,
        "out['started'] = True\nout['namespace'] = os.readlink('/proc/self/ns/mnt')\n",
    );

    assert_eq!(out["started"], true, "{out}");
    // The refusal was played: the run is in the mount namespace of the test, and so of the
    // daemon, not in one of its own.
    let ours = std::fs::read_link("/proc/self/ns/mnt").unwrap();
    assert_eq!(out["namespace"], ours.to_str().unwrap(), "{out}");
    // The gap is said where the sandbox's state is: the daemon's, not the command line's, which
    // is not given the variable.
    let doctor = user
        .command(&["doctor", "--json"])
        .env_remove("FLOTTILLE_TEST_NO_USER_NAMESPACES")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(report["sandbox"]["private_mounts"], false, "{report}");
    assert!(
        report["warnings"].to_string().contains("no user namespace"),
        "{report}"
    );
    if user_namespaces() {
        let doctor = User::with_fake_agent().run(&["doctor", "--json"]);
        let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
        assert_eq!(report["sandbox"]["private_mounts"], true, "{report}");
    }
}

/// `~/.ssh` of the test's home a link to `dots/ssh`, with a config and a `known_hosts`.
fn linked_ssh(user: &User) -> PathBuf {
    let dots = user.path("dots/ssh");
    std::fs::create_dir_all(&dots).unwrap();
    std::fs::write(dots.join("config"), "Host forge\n  User git\n").unwrap();
    std::fs::write(dots.join("known_hosts"), "forge ssh-ed25519 AAAA\n").unwrap();
    std::fs::create_dir_all(user.path("home")).unwrap();
    std::os::unix::fs::symlink(&dots, user.path("home/.ssh")).unwrap();
    user.path("home/.ssh")
}

/// The error number of a write to a read-only file system.
const READ_ONLY: i64 = libc::EROFS as i64;

#[test]
fn ssh_and_where_it_leads_are_read_only_to_a_run() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    let ssh = linked_ssh(&user);
    let (mcp, place) = session(&user);
    let at = |name: &str| py_path(&ssh.join(name));

    let out = ran(
        &mcp,
        &place,
        &[
            attempt("create", &format!("open({}, 'w')", at("new"))),
            attempt("change", &format!("open({}, 'a').write('x')", at("config"))),
            attempt(
                "rename",
                &format!("os.rename({}, {})", at("config"), at("config.old")),
            ),
            attempt("remove", &format!("os.remove({})", at("known_hosts"))),
            attempt("chmod", &format!("os.chmod({}, 0o666)", at("config"))),
            attempt(
                "through",
                &format!(
                    "open({}, 'w')",
                    py_path(&user.path("dots/ssh/through-the-target"))
                ),
            ),
            // The rest of the home stays the run's.
            attempt(
                "home",
                &format!("open({}, 'w')", py_path(&user.path("home/note"))),
            ),
        ]
        .concat(),
    );

    for refused in ["create", "change", "rename", "remove", "chmod", "through"] {
        assert_eq!(out[refused], READ_ONLY, "{refused}: {out}");
    }
    assert_eq!(out["home"], "done", "{out}");
}

#[test]
fn links_in_ssh_and_the_files_its_config_includes_are_read_only_to_a_run() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    let ssh = user.path("home/.ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::create_dir_all(user.path("dots/conf.d")).unwrap();
    std::fs::write(user.path("dots/key"), "a key\n").unwrap();
    std::os::unix::fs::symlink(user.path("dots/key"), ssh.join("id_linked")).unwrap();
    std::fs::write(
        ssh.join("config"),
        format!("Include {}/*.conf\n", user.path("dots/conf.d").display()),
    )
    .unwrap();
    std::fs::write(user.path("dots/conf.d/forge.conf"), "Host forge\n").unwrap();
    let (mcp, place) = session(&user);

    let out = ran(
        &mcp,
        &place,
        &[
            attempt(
                "linked",
                &format!("open({}, 'a').write('x')", py_path(&user.path("dots/key"))),
            ),
            attempt(
                "included",
                &format!(
                    "open({}, 'a').write('ProxyCommand x')",
                    py_path(&user.path("dots/conf.d/forge.conf"))
                ),
            ),
            // Beside them, a file of the same folder is the run's.
            attempt(
                "beside",
                &format!("open({}, 'w')", py_path(&user.path("dots/other"))),
            ),
        ]
        .concat(),
    );

    assert_eq!(out["linked"], READ_ONLY, "{out}");
    assert_eq!(out["included"], READ_ONLY, "{out}");
    assert_eq!(out["beside"], "done", "{out}");
}

#[test]
fn the_ssh_of_home_and_of_the_account_are_both_read_only_to_a_run() {
    if skipped_without_user_namespaces() {
        return;
    }
    let account = tempfile::tempdir().unwrap();
    let user = User::with_fake_agent().with_allowing_judge().with_env(
        "FLOTTILLE_TEST_ACCOUNT_HOME",
        account.path().to_str().unwrap(),
    );
    std::fs::create_dir_all(user.path("home/.ssh")).unwrap();
    std::fs::create_dir_all(account.path().join(".ssh")).unwrap();
    let (mcp, place) = session(&user);

    let out = ran(
        &mcp,
        &place,
        &[
            attempt(
                "home",
                &format!("open({}, 'w')", py_path(&user.path("home/.ssh/config"))),
            ),
            attempt(
                "account",
                &format!(
                    "open({}, 'w')",
                    py_path(&account.path().join(".ssh/config"))
                ),
            ),
        ]
        .concat(),
    );

    assert_eq!(out["home"], READ_ONLY, "{out}");
    assert_eq!(out["account"], READ_ONLY, "{out}");
}

#[test]
fn a_run_still_reads_ssh() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    let ssh = linked_ssh(&user);
    let (mcp, place) = session(&user);

    let out = ran(
        &mcp,
        &place,
        &format!(
            "out['config'] = open({}).read()\nout['known_hosts'] = open({}).read()\n",
            py_path(&ssh.join("config")),
            py_path(&ssh.join("known_hosts"))
        ),
    );

    assert_eq!(out["config"], "Host forge\n  User git\n", "{out}");
    assert_eq!(out["known_hosts"], "forge ssh-ed25519 AAAA\n", "{out}");
}

/// The error number of a change to a mount point: what cannot be renamed nor removed.
const PINNED: i64 = libc::EBUSY as i64;

/// `~/.ssh/config` of the test's home including `dots/conf.d/*.conf` (`forge.conf` is there) and
/// `dots/later/forge.conf`, which is not there yet.
fn included_from_dots(user: &User) {
    let ssh = user.path("home/.ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::create_dir_all(user.path("dots/conf.d")).unwrap();
    std::fs::create_dir_all(user.path("dots/later")).unwrap();
    std::fs::write(user.path("dots/conf.d/forge.conf"), "Host forge\n").unwrap();
    std::fs::write(
        ssh.join("config"),
        format!(
            "Include {}/conf.d/*.conf\nInclude {}/later/forge.conf\n",
            user.path("dots").display(),
            user.path("dots").display()
        ),
    )
    .unwrap();
}

#[test]
fn a_run_cannot_rename_a_folder_on_the_way_to_an_included_file() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    included_from_dots(&user);
    let (mcp, place) = session(&user);
    let at = |sub: &str| py_path(&user.path(sub));

    let out = ran(
        &mcp,
        &place,
        &[
            attempt(
                "dots",
                &format!("os.rename({}, {})", at("dots"), at("dots-old")),
            ),
            attempt(
                "folder",
                &format!("os.rename({}, {})", at("dots/conf.d"), at("dots/old")),
            ),
            attempt("remove", &format!("os.rmdir({})", at("dots/later"))),
            // A folder beside them is the run's.
            attempt("beside", &format!("os.mkdir({})", at("dots/other"))),
        ]
        .concat(),
    );

    assert_eq!(out["dots"], PINNED, "{out}");
    assert_eq!(out["folder"], PINNED, "{out}");
    assert_ne!(out["remove"], "done", "{out}");
    assert_eq!(out["beside"], "done", "{out}");
}

#[test]
fn a_run_cannot_add_a_file_to_an_included_folder() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    included_from_dots(&user);
    let (mcp, place) = session(&user);

    let out = ran(
        &mcp,
        &place,
        &attempt(
            "added",
            &format!(
                "open({}, 'w').write('ProxyCommand x')",
                py_path(&user.path("dots/conf.d/evil.conf"))
            ),
        ),
    );

    assert_eq!(out["added"], READ_ONLY, "{out}");
}

#[test]
fn a_run_cannot_create_a_missing_included_file() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    included_from_dots(&user);
    let (mcp, place) = session(&user);

    let out = ran(
        &mcp,
        &place,
        &attempt(
            "created",
            &format!(
                "open({}, 'w').write('ProxyCommand x')",
                py_path(&user.path("dots/later/forge.conf"))
            ),
        ),
    );

    assert_eq!(out["created"], READ_ONLY, "{out}");
}

#[test]
fn a_run_cannot_replace_the_ssh_link() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    let ssh = linked_ssh(&user);
    let (mcp, place) = session(&user);

    let out = ran(
        &mcp,
        &place,
        &[
            attempt("remove", &format!("os.remove({})", py_path(&ssh))),
            attempt(
                "rename",
                &format!(
                    "os.rename({}, {})",
                    py_path(&ssh),
                    py_path(&user.path("home/old"))
                ),
            ),
            format!(
                "out['config'] = open({}).read()\n",
                py_path(&ssh.join("config"))
            ),
        ]
        .concat(),
    );

    assert_eq!(out["remove"], PINNED, "{out}");
    assert_eq!(out["rename"], PINNED, "{out}");
    // The link still leads where it did.
    assert_eq!(out["config"], "Host forge\n  User git\n", "{out}");
}

/// What `flottille doctor` says of the SSH agent: the daemon's `sandbox.ssh_agent`.
fn ssh_agent_said(user: &User) -> Value {
    let doctor = user.run(&["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    report["sandbox"]["ssh_agent"].clone()
}

#[test]
fn the_ssh_agent_is_lent_to_nothing_where_an_include_s_folder_is_missing() {
    let user = User::with_fake_agent();
    let ssh = user.path("home/.ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(ssh.join("config"), "Host forge\n").unwrap();
    assert_eq!(
        ssh_agent_said(&user)["lent"],
        true,
        "{}",
        ssh_agent_said(&user)
    );

    // A folder a run could make, and then the file in it.
    std::fs::write(
        ssh.join("config"),
        format!("Include {}/nowhere/*.conf\n", user.path("dots").display()),
    )
    .unwrap();

    let said = ssh_agent_said(&user);
    assert_eq!(said["lent"], false, "{said}");
    assert!(said["why"].to_string().contains("nowhere"), "{said}");
}

#[test]
fn the_ssh_agent_is_lent_to_nothing_where_an_include_is_in_the_home_itself() {
    let user = User::with_fake_agent();
    let ssh = user.path("home/.ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(ssh.join("config"), "Include ~/forge.conf\n").unwrap();

    let said = ssh_agent_said(&user);

    assert_eq!(said["lent"], false, "{said}");
    assert!(said["why"].to_string().contains("forge.conf"), "{said}");
    let doctor = user.run(&["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert!(
        report["warnings"].to_string().contains("forge.conf"),
        "{report}"
    );
}

/// The most memory the process `pid` has held, in bytes.
fn peak_memory(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let line = status
        .lines()
        .find(|line| line.starts_with("VmHWM:"))
        .unwrap();
    let kib: u64 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    kib * 1024
}

#[test]
fn a_fifo_or_a_huge_file_in_an_include_does_not_hold_a_run() {
    let user = User::with_fake_agent().with_allowing_judge();
    let ssh = user.path("home/.ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    let (mcp, place) = session(&user);
    let pipe = user.path("dots/pipe");
    let huge = user.path("dots/huge");
    std::fs::create_dir_all(user.path("dots")).unwrap();
    let c = std::ffi::CString::new(pipe.to_str().unwrap()).unwrap();
    // SAFETY: a valid C string.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    // Sparse: nothing is written to the disk.
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(512 << 20)
        .unwrap();
    std::fs::write(
        ssh.join("config"),
        format!("Include {} {}\n", pipe.display(), huge.display()),
    )
    .unwrap();
    // Whatever waits on the pipe is let go after a while, so that a failure is a failure and
    // not a test that never ends.
    let freeing = pipe.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            use std::os::unix::fs::OpenOptionsExt;
            if std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&freeing)
                .is_ok()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    let pid = daemon_pid(&user);
    let started = Instant::now();

    let out = ran(&mcp, &place, "out['started'] = True");

    assert_eq!(out["started"], true, "{out}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the run took {:?}",
        started.elapsed()
    );
    assert!(
        peak_memory(pid) < 300 << 20,
        "the daemon held {} bytes",
        peak_memory(pid)
    );
}

#[test]
fn a_run_cannot_connect_to_a_session_bus_outside_the_runtime_folder() {
    if skipped_without_user_namespaces() {
        return;
    }
    let bus_folder = tempfile::tempdir().unwrap();
    let bus = bus_folder.path().join("dbus-test");
    let _listening = std::os::unix::net::UnixListener::bind(&bus).unwrap();
    let user = User::with_fake_agent().with_allowing_judge().with_env(
        "DBUS_SESSION_BUS_ADDRESS",
        &format!("unix:path={},guid=0123456789abcdef", bus.display()),
    );
    let (mcp, place) = session(&user);

    let out = ran(&mcp, &place, &connect("bus", bus.to_str().unwrap()));

    assert_ne!(out["bus"], "connected", "{out}");
}
