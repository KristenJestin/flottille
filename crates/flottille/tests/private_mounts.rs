//! The scenarios of the private mount namespace a run starts in (#976, `docs/vault.md`, "The
//! runtime folder"): the session bus out of reach by its path at any Landlock ABI, and a run that
//! still starts where the kernel refuses user namespaces, the gap said. The agent is the fake ACP
//! agent, in a throwaway repository, with temporary XDG folders, home and runtime folder; the run
//! is a small Python program.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp, server_of};
use common::{User, landlock_abi, user_namespaces};
use serde_json::{Value, json};

/// A project `app` over a throwaway repository, its workspace `w`, and the session of a root
/// agent working there in Auto, its judge allowing every call: a run in Full is the user's own,
/// unconfined (`docs/tools.md`). Its session, the workspace's folder.
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
    let output = user.run(&["agent", "mode", id, "auto"]);
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

#[test]
fn a_run_starts_once_the_session_bus_s_socket_is_gone() {
    if skipped_without_user_namespaces() {
        return;
    }
    let bus_folder = tempfile::tempdir().unwrap();
    let bus = bus_folder.path().join("dbus-test");
    let listening = std::os::unix::net::UnixListener::bind(&bus).unwrap();
    let user = User::with_fake_agent().with_allowing_judge().with_env(
        "DBUS_SESSION_BUS_ADDRESS",
        &format!("unix:path={},guid=0123456789abcdef", bus.display()),
    );
    let (mcp, place) = session(&user);
    // The bus the daemon found at its start ends, its socket removed.
    drop(listening);
    std::fs::remove_file(&bus).unwrap();

    let out = ran(&mcp, &place, "out['started'] = True");

    assert_eq!(out["started"], true, "{out}");
}

/// Whether a daemon of the user's runs.
fn daemon_runs(user: &User) -> bool {
    let program = std::fs::canonicalize(env!("CARGO_BIN_EXE_flottille")).unwrap();
    let state = format!("XDG_STATE_HOME={}", user.path("state").display());
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .any(|pid| {
            std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| exe == program)
                && std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|environ| {
                    environ
                        .split(|byte| *byte == 0)
                        .any(|variable| variable == state.as_bytes())
                })
        })
}

#[test]
fn doctor_starts_no_daemon() {
    let user = User::with_fake_agent();

    let doctor = user.run(&["doctor", "--json"]);

    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(
        report["sandbox"]["probed"], "no daemon running: probed by doctor",
        "{report}"
    );
    assert!(report["sandbox"]["landlock_abi"].is_number(), "{report}");
    assert!(!daemon_runs(&user), "doctor started a daemon");
}

/// A cookie file in the profile of the browser of the connection `browser`, where the daemon
/// keeps it (`docs/extensions.md`, "Adapters"): made by the test, as Chrome would.
fn browser_cookies(user: &User) -> PathBuf {
    let cookies = user.path("state/flottille/browsers/browser/profile/Default/Cookies");
    std::fs::create_dir_all(cookies.parent().unwrap()).unwrap();
    std::fs::write(&cookies, "session=signed-in").unwrap();
    cookies
}

/// The Python that reads the file at `path` without its path in the run's line nor in the
/// program as written (reversed): `read`, or the error's class, under `out[name]`.
fn read_unnamed(name: &str, path: &Path) -> String {
    let reversed: String = path.to_str().unwrap().chars().rev().collect();
    format!(
        "try:\n\
         \x20   open({reversed:?}[::-1]).read()\n\
         \x20   out[{name:?}] = 'read'\n\
         except OSError as e:\n\
         \x20   out[{name:?}] = type(e).__name__\n"
    )
}

#[test]
fn a_run_naming_a_browser_s_cookies_is_refused() {
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, _) = session(&user);
    let cookies = browser_cookies(&user);

    // In every mode, Full included: the gate refuses the path as a login's place, wherever the
    // kernel would let the run reach it.
    let ran = mcp.call("run", json!({"line": format!("cat {}", cookies.display())}));
    assert!(ran.error, "{}", ran.text);
    assert!(ran.text.contains("login's place"), "{}", ran.text);
    assert!(!ran.text.contains("signed-in"), "{}", ran.text);
    let read = mcp.call("fs_read", json!({"path": cookies}));
    assert!(read.error, "{}", read.text);
    assert!(read.text.contains("login's place"), "{}", read.text);
    // A line that reads every file below the state folder is refused too.
    let state = user.path("state/flottille");
    let grep = mcp.call(
        "run",
        json!({"line": format!("grep -r signed {}", state.display())}),
    );
    assert!(grep.error, "{}", grep.text);
    assert!(!grep.text.contains("signed-in"), "{}", grep.text);
}

#[test]
fn a_run_cannot_open_a_browser_s_profile_without_naming_it() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, place) = session(&user);
    let cookies = browser_cookies(&user);
    let other = user.path("state/flottille/other");
    std::fs::write(&other, "not a login").unwrap();

    let out = ran(
        &mcp,
        &place,
        &(read_unnamed("cookies", &cookies) + &read_unnamed("other", &other)),
    );

    // The browsers' folder is an empty one in the run's namespace, whatever Landlock's ABI.
    assert_eq!(out["cookies"], "FileNotFoundError", "{out}");
    // The rest of the state folder is not hidden by it.
    assert_eq!(out["other"], "read", "{out}");
}

/// A run renames `folder` (the state folder or one above it) to `moved`, then the next run, in a
/// namespace of its own, opens the browser's cookies under the new name, and puts the folder back.
fn renamed_then_read(folder: &str, moved: &str) {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    let (mcp, place) = session(&user);
    let cookies = browser_cookies(&user);
    let from = user.path(folder);
    let to = user.path(moved);
    let under = to.join(cookies.strip_prefix(&from).unwrap());
    let reversed = |path: &Path| path.to_str().unwrap().chars().rev().collect::<String>();
    let rename = |name: &str, from: &Path, to: &Path| {
        format!(
            "try:\n\
             \x20   os.rename({:?}[::-1], {:?}[::-1])\n\
             \x20   out[{name:?}] = 'renamed'\n\
             except OSError as e:\n\
             \x20   out[{name:?}] = type(e).__name__\n",
            reversed(from),
            reversed(to)
        )
    };

    let first = ran(&mcp, &place, &rename("renamed", &from, &to));
    let out = ran(
        &mcp,
        &place,
        &(read_unnamed("cookies", &under) + &rename("back", &to, &from)),
    );

    // The folders on the way to the browsers' are kept where they are, as `~/.ssh`'s: the
    // rename fails, and the profile is under no other name for the next run to open.
    assert_ne!(first["renamed"], "renamed", "{first}");
    assert_ne!(out["cookies"], "read", "{out}");
    assert!(cookies.is_file());
}

#[test]
fn a_run_that_renames_the_state_folder_still_cannot_read_a_browser_s_profile() {
    renamed_then_read("state/flottille", "state/moved");
}

#[test]
fn a_run_that_renames_a_folder_above_the_state_folder_still_cannot_read_a_browser_s_profile() {
    renamed_then_read("state", "moved");
}

#[test]
fn a_run_cannot_open_a_browser_s_profile_where_user_namespaces_are_refused() {
    if landlock_abi() < 2 {
        eprintln!(
            "skipped: this kernel's Landlock (ABI {}) is not used",
            landlock_abi()
        );
        return;
    }
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_env("FLOTTILLE_TEST_NO_USER_NAMESPACES", "1");
    let (mcp, place) = session(&user);
    let cookies = browser_cookies(&user);

    let out = ran(&mcp, &place, &read_unnamed("cookies", &cookies));

    // No namespace: Landlock denies the folder itself.
    assert_eq!(out["cookies"], "PermissionError", "{out}");
}
