//! Where a session runs, in the window (#1179, `docs/remote.md`, "Roles"): the place picker of the
//! start composer offers the machines that can run the project (`remote.machines` with the
//! project and the agent, what `flottille remote members --runners --project --agent` asks), and
//! choosing one shows that machine. On GPUI's test platform in front of two real daemons and a hub
//! served in the test, each with its own temporary HOME and XDG folders and a fake agent. No
//! window opens on a screen.

use crate::common;
use crate::remote::{Machine, pc_and_server_on};

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use common::User;
use flottille_link::{DeviceKey, Request};
use flottille_window::Shell;
use flottille_window::device::Device;
use flottille_window::link::{Connect, Sign};
use flottille_window::shell::Selection;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const WAIT: Duration = Duration::from_secs(60);

/// Each scenario runs two daemons and a hub: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A throwaway repository with one commit, registered as the project `name` of `user`.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "x\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn signing(machine: &Machine) -> Sign {
    let folder = machine.folder();
    Arc::new(move |target, request| {
        let key = DeviceKey::load(&crate::remote::NoKeyring, &folder)
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .ok_or_else(|| std::io::Error::other("no device key"))?;
        let signed = Request::sign(
            &key,
            &request.id,
            target,
            &request.method,
            &request.params,
            SystemTime::now(),
        )
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        serde_json::to_value(signed).map_err(std::io::Error::other)
    })
}

fn window<'a>(
    pc: &Machine,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: pc.user.path("os-release"),
        state_home: pc.user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: Connect = Arc::new(pc.user.connector());
    let device = Some(Device::in_folder(pc.folder()));
    let (shell, cx) =
        cx.add_window_view(|window, cx| Shell::new_polled_with(connect, device, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    shell.update(cx, |s, _| s.sign_with(signing(pc)));
    (shell, cx)
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.executor().advance_clock(Duration::from_millis(16));
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

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn present(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some())
}

fn click(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

#[gpui_kit::test]
fn the_composer_offers_the_machines_that_have_the_project(cx: &mut TestAppContext) {
    let _turn = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // The server is a runner whose Claude is signed in (its login file is there), so that it is
    // offered; the window's machine has the project and the same agent.
    let user = User::with_fake_agent()
        .with_env("FLOTTILLE_TEST_OFFER_MS", "300")
        .with_env("ANTHROPIC_API_KEY", "")
        .with_env("CLAUDE_CODE_OAUTH_TOKEN", "")
        .with_env("ANTHROPIC_AUTH_TOKEN", "");
    let claude = user.path("home/.claude");
    let user = user.with_env("CLAUDE_CONFIG_DIR", claude.to_str().unwrap());
    user.write_providers("[claude]\nenabled = true\n");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join(".credentials.json"), "{}").unwrap();
    let (_hub, pc, server) = pc_and_server_on(Machine::new(), user);
    std::fs::write(pc.user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    // `app` is on both machines; `site` on this one only: the server cannot run it.
    project(&pc.user, "app");
    project(&server.user, "app");
    project(&pc.user, "site");
    pc.wait_until("the server offers app and claude", |seen| {
        let offers = seen["machines"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|machine| machine["name"] == "server")
            .map(|machine| machine["offers"].clone())
            .unwrap_or_default();
        offers["projects"]
            .as_array()
            .is_some_and(|projects| projects.iter().any(|p| p["name"] == "app"))
            && offers["agents"]
                .as_array()
                .is_some_and(|agents| agents.iter().any(|a| a == "claude"))
    });

    let (shell, cx) = window(&pc, cx);
    until(&shell, cx, "the server is read", |cx| {
        shell.read_with(cx, |s, cx| {
            s.machines()
                .read(cx)
                .machines()
                .and_then(|read| read.machine("server"))
                .is_some_and(|server| server.online())
        })
    });
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("site".into()), window, cx)
    });
    until(&shell, cx, "the start dialog of site is there", |cx| {
        present(cx, "place-picker")
    });
    assert!(
        !present(cx, "machine-picker"),
        "the server has no project site: no picker"
    );

    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project("app".into()), window, cx)
    });
    until(&shell, cx, "the machine picker of app is there", |cx| {
        present(cx, "machine-picker")
    });
    click(cx, "machine-picker");
    until(&shell, cx, "the server is offered", |cx| {
        present(cx, "machine-choice-server")
    });
    // Choosing it shows that machine, where the session then starts.
    click(cx, "machine-choice-server");
    until(&shell, cx, "the window shows the server", |cx| {
        shell.read_with(&*cx, |s, _| s.machine_shown().as_deref() == Some("server"))
    });
}
