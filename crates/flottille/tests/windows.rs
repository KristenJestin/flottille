//! The scenarios of a project whose agents may open windows on the user's screen (#955): a run
//! of such a project gets a runtime folder of its own holding the session's Wayland socket alone,
//! removed when the run ends; a run of another project gets no display; and only the user turns
//! the setting on. The agents are the fake ACP agent, in throwaway repositories, temporary XDG
//! folders and a temporary runtime folder holding a fake `wayland-0` socket.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp, server_of};
use common::{User, landlock_abi};
use serde_json::{Value, json};

/// A user whose daemon runs in a desktop session: a Wayland socket in its runtime folder, an X11
/// display and a session bus beside it.
fn user_with_display() -> User {
    User::with_fake_agent()
        .with_allowing_judge()
        .with_env("WAYLAND_DISPLAY", "wayland-0")
        .with_env("DISPLAY", ":7")
        .with_env("XAUTHORITY", "/home/someone/.Xauthority")
}

/// The session's Wayland socket, in the user's runtime folder, with the session bus's socket
/// beside it: what a run must not be given.
fn wayland_socket(user: &User) -> std::os::unix::net::UnixListener {
    let runtime = user.path("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(runtime.join("bus"), "").unwrap();
    std::os::unix::net::UnixListener::bind(runtime.join("wayland-0")).unwrap()
}

/// A project `app` over a throwaway repository, its workspace `w`, and the session of a root
/// agent working there in Full, so that nothing is held: the agent's id, its session, the
/// workspace's folder.
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
    eventually("the workspace", || {
        user.call("workspace.show", json!({"project": "app", "name": "w"}))
            .unwrap()["state"]
            == "ready"
    });
    let place = PathBuf::from(made["folder"].as_str().unwrap());
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "start the app"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    let (url, authorization) = server_of(user, opened);
    let output = user.run(&["agent", "mode", &id, "full"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (id, Mcp::initialize(&url, &authorization), place)
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
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

fn refused(answer: Answer) -> String {
    assert!(answer.error, "not refused: {}", answer.text);
    answer.text
}

/// `flottille project edit app --windows <state>`, which must succeed.
fn windows(user: &User, state: &str) {
    let output = user.run(&["project", "edit", "app", "--windows", state]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// What a run of the agent sees of its display: the variables, and what its runtime folder holds
/// (none without one), as a small program of the workspace prints them.
fn seen(mcp: &Mcp, place: &Path) -> Value {
    std::fs::write(
        place.join("seen.py"),
        "import json, os\n\
         folder = os.environ.get('XDG_RUNTIME_DIR')\n\
         names = ['XDG_RUNTIME_DIR', 'WAYLAND_DISPLAY', 'DISPLAY', 'XAUTHORITY',\n\
             'DBUS_SESSION_BUS_ADDRESS']\n\
         print(json.dumps({'env': {name: os.environ.get(name) for name in names},\n\
             'files': sorted(os.listdir(folder)) if folder else None}))\n",
    )
    .unwrap();
    let ran = done(mcp.call("run", json!({"line": "python3 seen.py"})));
    assert!(ran.contains("exited with code 0"), "{ran}");
    let line = ran
        .lines()
        .find(|line| line.trim_start().starts_with("{\"env\""))
        .unwrap_or_else(|| panic!("{ran}"));
    serde_json::from_str(line.trim()).unwrap()
}

/// Whether a run reaches a listening socket: by its path, under the user's X11 socket folder
/// (the test's own), and by an abstract name; for each, `connected` or the error.
fn reached(mcp: &Mcp, place: &Path, path: &Path, abstract_name: &str) -> Value {
    std::fs::write(
        place.join("reach.py"),
        format!(
            "import json, socket\n\
             out = {{}}\n\
             for name, address in [('path', {path:?}), ('abstract', '\\0' + {abstract_name:?})]:\n\
             \x20   s = socket.socket(socket.AF_UNIX)\n\
             \x20   try:\n\
             \x20       s.connect(address)\n\
             \x20       out[name] = 'connected'\n\
             \x20   except OSError as e:\n\
             \x20       out[name] = type(e).__name__\n\
             print(json.dumps({{'reached': out}}))\n",
            path = path.to_str().unwrap(),
        ),
    )
    .unwrap();
    let ran = done(mcp.call("run", json!({"line": "python3 reach.py"})));
    let line = ran
        .lines()
        .find(|line| line.trim_start().starts_with("{\"reached\""))
        .unwrap_or_else(|| panic!("{ran}"));
    serde_json::from_str::<Value>(line.trim()).unwrap()["reached"].clone()
}

/// An X11 server of the session, as the test fakes it: its socket `X7` in the user's X11 socket
/// folder (the test's own, `FLOTTILLE_X11_SOCKETS`), and an abstract socket of a name of its
/// own (Landlock's scope denies every abstract socket made outside the run alike).
fn x11_server(
    user: &User,
) -> (
    std::os::unix::net::UnixListener,
    std::os::unix::net::UnixListener,
    PathBuf,
    String,
) {
    use std::os::linux::net::SocketAddrExt;
    let folder = user.path("x11");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("X7");
    let by_path = std::os::unix::net::UnixListener::bind(&path).unwrap();
    // The user's temporary folder has a name of its own: so has the socket.
    let unique = format!(
        "flottille-test-x11-{}",
        user.path("").file_name().unwrap().to_string_lossy()
    );
    let address = std::os::unix::net::SocketAddr::from_abstract_name(unique.as_bytes()).unwrap();
    let by_name = std::os::unix::net::UnixListener::bind_addr(&address).unwrap();
    (by_path, by_name, path, unique)
}

fn setting(user: &User) -> Value {
    user.call("project.settings", json!({"name": "app"}))
        .unwrap()["windows"]
        .clone()
}

#[test]
fn a_run_of_a_project_that_allows_windows_gets_the_display_alone() {
    let user = user_with_display();
    let _wayland = wayland_socket(&user);
    let (_, mcp, place) = session(&user);
    windows(&user, "on");
    assert_eq!(setting(&user), json!(true));

    let seen = seen(&mcp, &place);

    let folder = seen["env"]["XDG_RUNTIME_DIR"].as_str().unwrap_or_default();
    let display = user.path("runtime/flottille/display");
    assert!(
        Path::new(folder).parent() == Some(display.as_path()),
        "{seen}"
    );
    assert_eq!(seen["files"], json!(["wayland-0"]), "{seen}");
    assert_eq!(seen["env"]["WAYLAND_DISPLAY"], "wayland-0", "{seen}");
    // X11 only without Wayland; never the session bus.
    assert_eq!(seen["env"]["DISPLAY"], Value::Null, "{seen}");
    assert_eq!(
        seen["env"]["DBUS_SESSION_BUS_ADDRESS"],
        Value::Null,
        "{seen}"
    );

    // Its folder alone: the real runtime folder and the other display folders are out of reach.
    if landlock_abi() < 2 {
        eprintln!(
            "skipped: this kernel's Landlock (ABI {}) is not used",
            landlock_abi()
        );
        return;
    }
    for kept in [user.path("runtime"), display] {
        let listed = listed(&mcp, &place, &kept);
        assert_eq!(listed, "PermissionError", "{}: {listed}", kept.display());
    }
}

/// Whether a run lists `folder`: `listed` or the error.
fn listed(mcp: &Mcp, place: &Path, folder: &Path) -> String {
    std::fs::write(
        place.join("list.py"),
        format!(
            "import os\n\
             try:\n\
             \x20   os.listdir({folder:?})\n\
             \x20   print('[listed]')\n\
             except OSError as e:\n\
             \x20   print('[' + type(e).__name__ + ']')\n",
            folder = folder.to_str().unwrap(),
        ),
    )
    .unwrap();
    let ran = done(mcp.call("run", json!({"line": "python3 list.py"})));
    let line = ran
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with('[') && line.ends_with(']'))
        .unwrap_or_else(|| panic!("{ran}"));
    line.trim_matches(['[', ']']).to_owned()
}

#[test]
fn a_run_of_a_project_that_does_not_gets_no_display() {
    let user = user_with_display();
    let _wayland = wayland_socket(&user);
    let (_, mcp, place) = session(&user);
    // Off by default, and off again once turned on.
    assert_eq!(setting(&user), json!(false));
    for state in [None, Some("on"), Some("off")] {
        if let Some(state) = state {
            windows(&user, state);
        }
        if state == Some("on") {
            continue;
        }
        let seen = seen(&mcp, &place);
        for variable in [
            "XDG_RUNTIME_DIR",
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            assert_eq!(seen["env"][variable], Value::Null, "{variable}: {seen}");
        }
        assert_eq!(seen["files"], Value::Null, "{seen}");
    }
    assert_eq!(setting(&user), json!(false));
}

#[test]
fn a_run_gets_no_xauthority() {
    let user = user_with_display();
    let _wayland = wayland_socket(&user);
    let (_, mcp, place) = session(&user);

    // Neither without windows nor with Wayland's.
    for state in [None, Some("on")] {
        if let Some(state) = state {
            windows(&user, state);
        }
        let seen = seen(&mcp, &place);
        assert_eq!(seen["env"]["XAUTHORITY"], Value::Null, "{state:?}: {seen}");
    }
}

#[test]
fn a_run_without_windows_cannot_reach_x11() {
    let user = user_with_display();
    let _wayland = wayland_socket(&user);
    let (_listening, _named, socket, name) = x11_server(&user);
    let (_, mcp, place) = session(&user);

    // Off, and on with Wayland: X11 is the session's only without Wayland.
    for state in [None, Some("on")] {
        if let Some(state) = state {
            windows(&user, state);
        }
        let reached = reached(&mcp, &place, &socket, &name);
        // By its path, the folder is an empty one in the run's namespace.
        if common::user_namespaces() {
            assert_eq!(reached["path"], "FileNotFoundError", "{state:?}: {reached}");
        } else {
            eprintln!("skipped: this kernel refuses user namespaces to this user");
        }
        // By its abstract name, from Landlock's sixth ABI.
        match landlock_abi() {
            abi if abi >= 6 => {
                assert_eq!(
                    reached["abstract"], "PermissionError",
                    "{state:?}: {reached}"
                );
            }
            abi => eprintln!("skipped: this kernel's Landlock (ABI {abi}) has no abstract scope"),
        }
    }
}

#[test]
fn the_shared_display_folder_is_still_reachable_in_the_namespace() {
    if !common::user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return;
    }
    let user = user_with_display();
    let _wayland = wayland_socket(&user);
    let (_, mcp, place) = session(&user);
    windows(&user, "on");

    let seen = seen(&mcp, &place);
    let folder = PathBuf::from(seen["env"]["XDG_RUNTIME_DIR"].as_str().unwrap());
    let reached = reached(&mcp, &place, &folder.join("wayland-0"), "none");
    let the_session_s = reached_path(&mcp, &place, &user.path("runtime/wayland-0"));

    // Its own folder, under the runtime folder the namespace empties, put back in it.
    assert_eq!(seen["files"], json!(["wayland-0"]), "{seen}");
    assert_eq!(reached["path"], "connected", "{reached}");
    // Beside it, the session's own socket is not there.
    assert_eq!(the_session_s, "FileNotFoundError");
}

/// [`reached`] by the path alone.
fn reached_path(mcp: &Mcp, place: &Path, path: &Path) -> Value {
    reached(mcp, place, path, "none")["path"].clone()
}

#[test]
fn a_run_given_x11_reaches_it() {
    // A session without Wayland: a run of a project that allows windows gets X11's display.
    let user = User::with_fake_agent()
        .with_allowing_judge()
        .with_env("DISPLAY", ":7")
        .with_env("XAUTHORITY", "/home/someone/.Xauthority");
    let (_listening, _named, socket, name) = x11_server(&user);
    let (_, mcp, place) = session(&user);
    windows(&user, "on");

    let seen = seen(&mcp, &place);
    let reached = reached(&mcp, &place, &socket, &name);

    assert_eq!(seen["env"]["DISPLAY"], ":7", "{seen}");
    // Given, its value masked in what the run prints, as every key's.
    assert_eq!(seen["env"]["XAUTHORITY"], "[masked]", "{seen}");
    assert_eq!(reached["path"], "connected", "{reached}");
    assert_eq!(reached["abstract"], "connected", "{reached}");
}

#[test]
fn the_run_s_runtime_folder_goes_when_the_run_ends() {
    let user = user_with_display();
    let _wayland = wayland_socket(&user);
    let (_, mcp, place) = session(&user);
    windows(&user, "on");

    let seen = seen(&mcp, &place);

    let folder = PathBuf::from(seen["env"]["XDG_RUNTIME_DIR"].as_str().unwrap());
    // The run has ended: its folder is gone, and none is left beside it.
    assert!(!folder.exists(), "{}", folder.display());
    let display = user.path("runtime/flottille/display");
    let left: Vec<_> = std::fs::read_dir(&display)
        .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    // The session's own socket is untouched.
    assert!(user.path("runtime/wayland-0").exists());
}

#[test]
fn an_agent_cannot_turn_windows_on() {
    let user = user_with_display();
    let _wayland = wayland_socket(&user);
    let (_, mcp, place) = session(&user);
    let file = || std::fs::read_to_string(user.project_file("app")).unwrap();
    let before = file();

    // Every tool of an agent that changes the project, in Full, given the setting: each refuses
    // it as a field it does not know, and nothing is saved.
    for (tool, args) in [
        (
            "project",
            json!({"action": "set_settings", "windows": true}),
        ),
        (
            "project",
            json!({"action": "set_settings", "windows": true,
                "repositories": [{"path": ".", "base": ["main"]}]}),
        ),
        (
            "project",
            json!({"action": "set_settings", "repositories": [{"path": ".", "windows": true}]}),
        ),
        (
            "project",
            json!({"action": "set_rules", "text": "Be brief.", "windows": true}),
        ),
        (
            "tasks",
            json!({"action": "add", "name": "app", "line": "true", "windows": true}),
        ),
        (
            "setup",
            json!({"action": "add", "step": {"run": "true", "windows": true}}),
        ),
        (
            "skills",
            json!({"action": "add", "name": "s", "description": "d", "text": "t",
                "windows": true}),
        ),
        (
            "commands",
            json!({"action": "add", "name": "c", "description": "d", "instructions": "i",
                "windows": true}),
        ),
    ] {
        let said = refused(mcp.call(tool, args.clone()));
        assert!(said.contains("windows"), "{tool} {args}: {said}");
    }
    // Nor by writing the project's file: Flottille's own folders are refused in every mode.
    let edited = format!("windows = true\n{before}");
    let said = refused(mcp.call(
        "fs_write",
        json!({"path": user.project_file("app"), "content": edited}),
    ));
    assert!(said.contains("Flottille's own folders"), "{said}");

    assert_eq!(file(), before);
    assert_eq!(setting(&user), json!(false));
    let seen = seen(&mcp, &place);
    assert_eq!(seen["env"]["XDG_RUNTIME_DIR"], Value::Null, "{seen}");
    // The user does.
    windows(&user, "on");
    assert!(file().contains("windows = true"), "{}", file());
}

// The setting in the window: a labelled switch with its help, in the project's Tasks tab.

mod window {
    use super::*;
    use flottille_window::Shell;
    use flottille_window::project_settings::{ProjectSettingsPage, SECTIONS};
    use flottille_window::shell::Selection;
    use gpui_design_system::omarchy::Environment;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
    use std::sync::Arc;

    fn until(
        shell: &Entity<Shell>,
        cx: &mut VisualTestContext,
        what: &str,
        check: impl Fn(&mut VisualTestContext) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
            cx.run_until_parked();
            cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
            if check(cx) {
                return;
            }
            assert!(Instant::now() < deadline, "{what} never happened");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Lets every motion end, then draws.
    fn settle(cx: &mut VisualTestContext) {
        for _ in 0..90 {
            cx.executor().advance_clock(Duration::from_millis(16));
            cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
        }
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// How many frames the window still asks for: none at rest.
    fn pending_frames(cx: &mut VisualTestContext) -> usize {
        cx.update(|window, cx| window.simulate_next_frame(cx))
    }

    fn label_of(id: &str, cx: &mut VisualTestContext) -> Option<String> {
        let id = id.to_owned();
        cx.update(|window, _| {
            window
                .try_find(id)
                .and_then(|found| found.label().map(str::to_owned))
        })
    }

    fn has(id: &str, cx: &mut VisualTestContext) -> bool {
        let id = id.to_owned();
        cx.update(|window, _| window.try_find(id).is_some_and(|found| found.visible()))
    }

    fn toasts(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<String> {
        shell.read_with(cx, |s, cx| {
            s.toasts()
                .read(cx)
                .toasts()
                .map(|toast| toast.title().to_owned())
                .collect()
        })
    }

    #[gpui_kit::test]
    fn the_windows_switch_is_labelled_explained_animated_and_saved(cx: &mut TestAppContext) {
        let user = User::with_fake_agent();
        std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
        let folder = user.path("work/app");
        user.git_init(&folder);
        user.call("project.add", json!({"folder": folder, "name": "app"}))
            .unwrap();
        let env = Environment {
            os_release: user.path("os-release"),
            state_home: user.path("state"),
        };
        cx.update(|cx| flottille_window::init_with(env, cx));
        let connect = Arc::new(user.connector());
        let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
        cx.simulate_resize(size(px(1280.), px(2400.)));
        shell.update_in(cx, |s, window, cx| {
            s.open(Selection::ProjectSettings("app".into()), window, cx)
        });
        let page = |cx: &mut VisualTestContext| -> Option<Entity<ProjectSettingsPage>> {
            shell.read_with(cx, |s, cx| {
                s.main().read(cx).project_settings_page().cloned()
            })
        };
        until(&shell, cx, "the settings are read", |cx| {
            page(cx).is_some_and(|page| page.read_with(cx, |page, cx| page.has_read(cx)))
        });
        let page = page(cx).unwrap();
        let tasks = SECTIONS
            .iter()
            .position(|(key, _)| *key == "tasks")
            .unwrap();
        page.update(cx, |page, cx| page.choose(tasks, cx));
        settle(cx);
        let file = || std::fs::read_to_string(user.project_file("app")).unwrap();

        // Off by default, with its label and what being on gives.
        assert!(has("project-windows", cx));
        assert_eq!(
            label_of("project-windows-label", cx).as_deref(),
            Some("Agents may open windows on my screen")
        );
        let help = label_of("project-windows-hint", cx).unwrap();
        assert!(help.contains("Wayland"), "{help}");
        assert!(help.contains("X11"), "{help}");
        assert!(help.contains("type into"), "{help}");
        assert!(!file().contains("windows"), "{}", file());
        assert_eq!(pending_frames(cx), 0, "nothing moves at rest");

        // On: the switch slides, the file says it, a toast says what it does.
        cx.update(|window, cx| window.click("project-windows", cx));
        until(&shell, cx, "the setting is saved", |_| {
            file().contains("windows = true")
        });
        assert!(pending_frames(cx) > 0, "the switch moves");
        settle(cx);
        assert_eq!(pending_frames(cx), 0, "the switch moved, then stopped");
        until(&shell, cx, "the toast says it", |cx| {
            toasts(&shell, cx).iter().any(|t| t == "Windows allowed")
        });

        // Off again: the key leaves the file.
        cx.update(|window, cx| window.click("project-windows", cx));
        until(&shell, cx, "the setting is saved off", |_| {
            !file().contains("windows")
        });
        settle(cx);
        assert_eq!(pending_frames(cx), 0);
    }
}

#[test]
fn a_run_makes_moves_and_uses_its_own_files_and_sockets_in_tmp() {
    // X11's folder in a stand-in of `/tmp` that holds no runtime folder, as the machine's does:
    // nothing ruled about X11 may take from a run what it makes in `/tmp` after it started.
    let user = user_with_display();
    let tmp = user.path("tmp");
    std::fs::create_dir_all(tmp.join(".X11-unix")).unwrap();
    let user = user.with_env(
        "FLOTTILLE_X11_SOCKETS",
        tmp.join(".X11-unix").to_str().unwrap(),
    );
    let (_, mcp, place) = session(&user);
    std::fs::write(
        place.join("scratch.py"),
        format!(
            "import os, socket\n\
             made = os.path.join({tmp:?}, 'scratch')\n\
             os.mkdir(made)\n\
             with open(os.path.join(made, 'a'), 'w') as f:\n\
             \x20   f.write('notes')\n\
             os.replace(os.path.join(made, 'a'), os.path.join(made, 'b'))\n\
             os.link(os.path.join(made, 'b'), os.path.join(made, 'c'))\n\
             listening = socket.socket(socket.AF_UNIX)\n\
             listening.bind(os.path.join(made, 'sock'))\n\
             listening.listen(1)\n\
             client = socket.socket(socket.AF_UNIX)\n\
             client.connect(os.path.join(made, 'sock'))\n\
             os.replace(os.path.join(made, 'b'), 'moved.txt')\n\
             print('[made, moved and connected]')\n",
            tmp = tmp.to_str().unwrap(),
        ),
    )
    .unwrap();

    let ran = done(mcp.call("run", json!({"line": "python3 scratch.py"})));

    // Only the ninth ABI ruled the way to X11's folder: below, this proves nothing more than it says.
    eprintln!("this kernel's Landlock ABI: {}", landlock_abi());
    assert!(ran.contains("[made, moved and connected]"), "{ran}");
    assert_eq!(
        std::fs::read_to_string(place.join("moved.txt")).unwrap(),
        "notes"
    );
}
