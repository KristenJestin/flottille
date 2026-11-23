//! The keys of remote (`docs/remote.md`, "Identity and keys") as the daemon and its agents see
//! them: the machine key the daemon makes at its first start with remote on, the device key of the
//! clients it never reads, and the key folder no agent reaches. Temporary HOME and XDG folders, the
//! in-memory keyring, fake ACP agents.

use crate::common;

use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp, server_of};
use common::{User, mode};
use serde_json::json;

/// Turns remote on in the daemon's configuration (`daemon.toml`), as the user does.
fn remote_on(user: &User) {
    std::fs::write(
        user.path("config/flottille/daemon.toml"),
        "[remote]\non = true\n",
    )
    .unwrap();
}

fn restart(user: &User) {
    assert!(user.run(&["daemon", "stop"]).status.success());
    user.status();
}

#[test]
fn the_machine_key_is_made_once_and_kept_private() {
    let user = User::new();
    let folder = user.path("state/flottille/remote");
    let key = folder.join("machine.key");

    // Off by default: nothing is made.
    user.status();
    assert!(!folder.exists(), "the key folder was made with remote off");

    remote_on(&user);
    restart(&user);
    assert_eq!(mode(folder.clone()), 0o700);
    assert_eq!(mode(key.clone()), 0o600);
    let made = std::fs::read(&key).unwrap();
    assert_eq!(made.len(), 32);
    let inode = std::fs::metadata(&key).unwrap().ino();

    // Read back at the next start, never made again.
    restart(&user);
    assert_eq!(std::fs::read(&key).unwrap(), made);
    assert_eq!(std::fs::metadata(&key).unwrap().ino(), inode);
    assert_eq!(mode(key), 0o600);
}

#[test]
fn a_machine_key_readable_by_others_is_refused() {
    let user = User::new();
    remote_on(&user);
    user.status();
    let key = user.path("state/flottille/remote/machine.key");
    let made = std::fs::read(&key).unwrap();
    assert!(user.run(&["daemon", "stop"]).status.success());
    std::fs::set_permissions(&key, PermissionsExt::from_mode(0o644)).unwrap();

    let output = user.run(&["daemon"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("{} is readable by others", key.display())),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("chmod 600 {}", key.display())),
        "{stderr}"
    );
    let hex: String = made.iter().map(|byte| format!("{byte:02x}")).collect();
    assert!(!stderr.contains(&hex[..8]), "{stderr}");
    // Left as it is: the user's own to put right.
    assert_eq!(std::fs::read(&key).unwrap(), made);
    assert_eq!(mode(key), 0o644);
}

/// An inotify instance watching the opens and reads of what is in `folder`, by any process.
fn watch_opens(folder: &Path) -> File {
    // SAFETY: inotify_init1 has no precondition; its result is checked.
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    assert!(fd >= 0, "{}", std::io::Error::last_os_error());
    // SAFETY: `fd` was just returned to this process and is owned by nothing else.
    let watcher = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    let path = CString::new(folder.as_os_str().as_bytes()).unwrap();
    // SAFETY: `path` is a valid C string that outlives the call.
    let watch =
        unsafe { libc::inotify_add_watch(fd, path.as_ptr(), libc::IN_OPEN | libc::IN_ACCESS) };
    assert!(watch >= 0, "{}", std::io::Error::last_os_error());
    watcher
}

/// The names of the files opened or read since the last call.
fn opened(mut watcher: &File) -> Vec<String> {
    let mut names = Vec::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = match watcher.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return names,
            Err(error) => panic!("{error}"),
        };
        // Each event: wd (i32), mask, cookie, len (u32), then `len` bytes of name, padded.
        let mut at = 0;
        while at + 16 <= read {
            let len = u32::from_ne_bytes(buffer[at + 12..at + 16].try_into().unwrap()) as usize;
            let name = &buffer[at + 16..at + 16 + len];
            let name = name.split(|byte| *byte == 0).next().unwrap_or_default();
            names.push(String::from_utf8_lossy(name).into_owned());
            at += 16 + len;
        }
    }
}

#[test]
fn the_daemon_never_reads_the_device_key() {
    let user = User::new();
    remote_on(&user);
    // The clients' device key on a machine with no Secret Service, beside the machine key.
    let folder = user.path("state/flottille/remote");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .unwrap();
    let device = folder.join("device.key");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&device)
        .unwrap();
    std::fs::write(&device, [7u8; 32]).unwrap();
    let watcher = watch_opens(&folder);

    // The daemon starts, makes its key, serves, stops, and starts again: it reads its key back.
    user.status();
    user.call("project.list", json!({})).unwrap();
    restart(&user);
    user.call("project.list", json!({})).unwrap();
    assert!(user.run(&["daemon", "stop"]).status.success());

    let seen = opened(&watcher);
    assert!(
        seen.iter().any(|name| name == "machine.key"),
        "the daemon's own reads were not seen: {seen:?}"
    );
    assert!(
        !seen.iter().any(|name| name == "device.key"),
        "the daemon opened the device key: {seen:?}"
    );
    // The watch sees an open of the device key: this test's own.
    std::fs::read(&device).unwrap();
    let seen = opened(&watcher);
    assert!(seen.iter().any(|name| name == "device.key"), "{seen:?}");

    // Nor does any of the daemon's code name what reaches the device key: its loader, its file,
    // its keyring item. Its limit: it reads names, not what the code does; a name built from
    // parts, or a crate the daemon links that loads the key itself, escapes it, which the watch
    // above is for. The command line shares these sources: its command that loads the device key
    // (`flottille remote setup`) is a client's, and left out.
    let clients = ["remote_command.rs"];
    let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in rust_files(&sources) {
        if file
            .file_name()
            .is_some_and(|name| clients.iter().any(|client| name == *client))
        {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for name in [
            "DeviceKey",
            "DEVICE_FILE",
            "DEVICE_LABEL",
            "DEVICE_ATTRIBUTES",
            "flottille_link::*",
            "device.key",
            "Flottille: device",
            "flottille-key",
        ] {
            assert!(
                !text.contains(name),
                "{} names {name}: the daemon never reads the device key",
                file.display()
            );
        }
    }
}

fn rust_files(folder: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files
}

/// A project `app` over a throwaway repository, its workspace `w`, and the session of a root agent
/// working there: the agent's id, its MCP client, the workspace's folder.
fn session(user: &User) -> (String, Mcp, PathBuf) {
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
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "add the login page"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    (
        id,
        Mcp::initialize(&url, &authorization),
        PathBuf::from(made["folder"].as_str().unwrap()),
    )
}

fn refused(what: &str, answer: Answer) -> String {
    assert!(answer.error, "{what}: not refused: {}", answer.text);
    assert!(
        answer.text.starts_with("refused: "),
        "{what}: {}",
        answer.text
    );
    answer.text
}

#[test]
fn an_agent_reading_or_writing_the_remote_folder_is_refused_in_every_mode() {
    let user = User::with_fake_agent().with_allowing_judge();
    remote_on(&user);
    let (id, mcp, place) = session(&user);
    let folder = user.path("state/flottille/remote");
    let machine = folder.join("machine.key");
    let device = folder.join("device.key");
    std::fs::write(&device, b"fake-device-key-of-the-clients-.").unwrap();
    std::fs::set_permissions(&device, PermissionsExt::from_mode(0o600)).unwrap();
    // The machines it trusts and the addresses declared by hand, beside the keys.
    let peers = folder.join("peers.json");
    let addresses = folder.join("addresses.json");
    std::fs::write(&peers, br#"{"machines": []}"#).unwrap();
    std::fs::write(&addresses, br#"{"addresses": []}"#).unwrap();
    let before = [
        std::fs::read(&machine).unwrap(),
        std::fs::read(&device).unwrap(),
        std::fs::read(&peers).unwrap(),
        std::fs::read(&addresses).unwrap(),
    ];
    // A link the agent could make in its place, to the key folder.
    std::os::unix::fs::symlink(&folder, place.join("keys")).unwrap();
    let stolen = place.join("stolen.key");
    let shown = |path: &Path| path.display().to_string();

    let calls = [
        ("fs_read", json!({"path": machine})),
        ("fs_read", json!({"path": device})),
        ("fs_read", json!({"path": "keys/machine.key"})),
        ("fs_list", json!({"path": folder})),
        ("fs_write", json!({"path": machine, "content": "replaced"})),
        (
            "fs_write",
            json!({"path": folder.join("other.key"), "content": "planted"}),
        ),
        (
            "fs_edit",
            json!({"path": device, "edits": [{"old": "fake", "new": "mine"}]}),
        ),
        ("run", json!({"line": format!("cat {}", shown(&machine))})),
        ("run", json!({"line": format!("base64 {}", shown(&device))})),
        (
            "run",
            json!({"line": format!("grep -r fake {}", shown(&folder))}),
        ),
        ("run", json!({"line": format!("ls -la {}", shown(&folder))})),
        (
            "run",
            json!({"line": format!("cp {} {}", shown(&machine), shown(&stolen))}),
        ),
        ("run", json!({"line": "cp README.md keys/machine.key"})),
        ("run", json!({"line": format!("rm -f {}", shown(&device))})),
        ("fs_read", json!({"path": peers})),
        ("fs_read", json!({"path": addresses})),
        ("fs_read", json!({"path": "keys/peers.json"})),
        (
            "fs_write",
            json!({"path": peers, "content": "{\"machines\": [{\"name\": \"mine\"}]}"}),
        ),
        (
            "fs_edit",
            json!({"path": addresses, "edits": [{"old": "[]", "new": "[\"192.0.2.66\"]"}]}),
        ),
        ("run", json!({"line": format!("cat {}", shown(&peers))})),
        (
            "run",
            json!({"line": format!("cp README.md {}", shown(&addresses))}),
        ),
    ];
    for mode in ["ask", "auto", "full"] {
        let output = user.run(&["agent", "mode", &id, mode]);
        assert!(output.status.success(), "{output:?}");
        for (tool, args) in &calls {
            let what = format!("{mode}: {tool} {args}");
            let said = refused(&what, mcp.call(tool, args.clone()));
            assert!(said.contains("login's place"), "{what}: {said}");
            assert!(said.contains("remote"), "{what}: {said}");
            assert!(!said.contains("fake-device"), "{what}: {said}");
        }
        assert!(
            user.call("approval.list", json!({})).unwrap()["requests"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{mode}: a call was held"
        );
    }
    assert_eq!(
        [
            std::fs::read(&machine).unwrap(),
            std::fs::read(&device).unwrap(),
            std::fs::read(&peers).unwrap(),
            std::fs::read(&addresses).unwrap(),
        ],
        before
    );
    assert!(!stolen.exists());
    assert!(!folder.join("other.key").exists());

    // Only Flottille's own `remote/` is a login's place: a project's folder of that name is read.
    std::fs::create_dir(place.join("remote")).unwrap();
    std::fs::write(place.join("remote/notes.md"), "the remote's notes\n").unwrap();
    let read = mcp.call("fs_read", json!({"path": "remote/notes.md"}));
    assert!(!read.error, "{}", read.text);
    assert!(read.text.contains("the remote's notes"), "{}", read.text);
}

#[test]
fn a_recursive_read_of_a_folder_above_the_keys_is_refused_in_every_mode() {
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_fake_escalation();
    remote_on(&user);
    let (id, mcp, place) = session(&user);
    let state = user.path("state/flottille");
    // A link the agent could make in its place, to the folder above the keys.
    std::os::unix::fs::symlink(&state, place.join("st")).unwrap();
    let s = state.display();
    let up = user.path("state");
    let up = up.display();
    let lines = [
        format!("grep -ra x {s}"),
        format!("grep -r -a x {up}"),
        format!("grep -R x {s}/remote/.."),
        format!("rg -uu -a x {s}"),
        format!("rg -a x /proc/self/root{s}"),
        "grep -ra x st".to_owned(),
        format!("diff -r -a {s} ."),
        format!("git -C {s} grep --no-index -a x"),
        format!("find {s} -type f -exec cat {{}} +"),
        format!("tar cf - {s}"),
        format!("cp -r {s} copy"),
        format!("env -C {up} grep -ra x"),
        format!("env --chdir={up} grep -ra x"),
        format!("env -C {up} git -C flottille grep --no-index -a x"),
    ];
    for mode in ["ask", "auto", "full"] {
        let output = user.run(&["agent", "mode", &id, mode]);
        assert!(output.status.success(), "{output:?}");
        for line in &lines {
            let what = format!("{mode}: {line}");
            let said = refused(&what, mcp.call("run", json!({ "line": line })));
            assert!(said.contains("Flottille's keys"), "{what}: {said}");
            assert!(said.contains("remote/"), "{what}: {said}");
        }
        assert!(
            user.call("approval.list", json!({})).unwrap()["requests"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{mode}: a call was held"
        );
    }
    assert!(!place.join("copy").exists());
    assert!(user.escalations().is_empty());

    // A listing shows names, never a byte: it runs, in Full as in Auto, and lists them.
    for mode in ["full", "auto"] {
        let output = user.run(&["agent", "mode", &id, mode]);
        assert!(output.status.success(), "{output:?}");
        let listed = mcp.call("run", json!({"line": format!("ls {s}")}));
        assert!(!listed.error, "{mode}: {}", listed.text);
        assert!(listed.text.contains("remote"), "{mode}: {}", listed.text);
    }
    // A search's pattern is not a folder: from an ordinary place, it runs.
    for line in ["grep -rn / .", "grep -rn '~' ."] {
        let searched = mcp.call("run", json!({ "line": line }));
        assert!(
            !searched.text.starts_with("refused: "),
            "{line}: {}",
            searched.text
        );
    }
}
