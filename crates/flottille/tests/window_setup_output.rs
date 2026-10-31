//! The scenarios of #173: a click on a step of a workspace's setup, on the project page, opens the
//! run drawer on what the step runs and writes. On GPUI's test platform in front of a real daemon,
//! with a throwaway user and repository. No window opens on a screen.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_core::workspace::StepState;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::project::{PlaceCard, ProjectPage};
use flottille_window::shell::Selection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn user() -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A project over a throwaway repository with a remote.
fn project(user: &User, name: &str) -> PathBuf {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    let remote = user.path(&format!("remotes/{name}.git"));
    user.git(
        &user.path(""),
        &["init", "--quiet", "--bare", remote.to_str().unwrap()],
    );
    user.git(
        &folder,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    user.git(&folder, &["push", "--quiet", "origin", "main"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
    folder
}

/// Adds a setup step that runs `body` as a script.
fn step(user: &User, project: &str, name: &str, body: &str) {
    let script = user.path(&format!("tools/{name}.sh"));
    std::fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::fs::write(&script, body).unwrap();
    let file = user.project_file(project);
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(&format!("\n[[setup]]\nrun = \"sh {}\"\n", script.display()));
    std::fs::write(file, text).unwrap();
}

fn wait_for(file: &Path) -> String {
    format!("while [ ! -e {} ]; do sleep 0.05; done\n", file.display())
}

fn window<'a>(
    user: &User,
    connect: Connect,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(1400.)));
    (shell, cx)
}

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
        // The drawer reads the output again each second of the window's clock.
        cx.executor().advance_clock(Duration::from_millis(100));
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
    for _ in 0..90 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn run_for(by: Duration, cx: &mut VisualTestContext) {
    let step = Duration::from_millis(16);
    let mut gone = Duration::ZERO;
    while gone < by {
        cx.executor().advance_clock(step);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += step;
    }
}

fn text_of(id: &str, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.to_owned();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn open_project(
    shell: &Entity<Shell>,
    project: &str,
    cx: &mut VisualTestContext,
) -> Entity<ProjectPage> {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::Project(project.into()), window, cx)
    });
    until(shell, cx, "the project's page is shown", |cx| {
        shell.read_with(cx, |s, cx| {
            s.main()
                .read(cx)
                .project_page()
                .is_some_and(|page| page.read(cx).name() == project)
        })
    });
    settle(cx);
    shell
        .read_with(cx, |s, cx| s.main().read(cx).project_page().cloned())
        .unwrap()
}

fn card_of(
    page: &Entity<ProjectPage>,
    name: &str,
    cx: &mut VisualTestContext,
) -> Option<Entity<PlaceCard>> {
    page.read_with(cx, |page, cx| page.card(&Some(name.to_owned()), cx))
}

/// The row of the first `Run` step in `state` on the card of `name`, once there is one.
fn run_row(
    page: &Entity<ProjectPage>,
    name: &str,
    state: StepState,
    cx: &mut VisualTestContext,
) -> Option<usize> {
    card_of(page, name, cx).and_then(|card| {
        card.read_with(cx, |c, _| {
            c.data()
                .steps
                .iter()
                .position(|s| s.verb == "Run" && s.state == state)
        })
    })
}

fn create(user: &User, name: &str) {
    user.call(
        "workspace.create",
        json!({"project": "atlas", "name": name}),
    )
    .unwrap();
}

fn shown_lines(page: &Entity<ProjectPage>, cx: &mut VisualTestContext) -> Option<Vec<String>> {
    let tasks = page.read_with(cx, |page, _| page.tasks().clone());
    tasks.read_with(cx, |t, cx| {
        t.panel().read(cx).shown().map(|o| o.lines.clone())
    })
}

#[gpui_kit::test]
fn a_running_setup_step_opens_the_run_drawer_and_its_output_streams_into_it(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    let go = user.path("go");
    step(
        &user,
        "atlas",
        "stream",
        &format!("echo first\n{}echo second\n", wait_for(&go)),
    );
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let page = open_project(&shell, "atlas", cx);
    create(&user, "onboarding");
    until(&shell, cx, "the step runs", |cx| {
        run_row(&page, "onboarding", StepState::Running, cx).is_some()
    });
    let at = run_row(&page, "onboarding", StepState::Running, cx).unwrap();
    settle(cx);

    cx.update(|window, cx| window.click(format!("setup-step-onboarding-{at}"), cx));
    until(&shell, cx, "the drawer shows what it wrote so far", |cx| {
        shown_lines(&page, cx).is_some_and(|lines| lines == ["first"])
    });
    settle(cx);

    cx.update(|window, _| assert!(window.find("run-drawer-panel").visible()));
    assert!(
        text_of("run-fact-command", cx).is_some_and(|line| line.contains("stream.sh")),
        "the command: {:?}",
        text_of("run-fact-command", cx)
    );
    assert!(
        text_of("run-fact-where", cx).is_some_and(|place| place.contains("onboarding")),
        "where it runs: {:?}",
        text_of("run-fact-where", cx)
    );
    assert!(
        text_of("run-drawer-sub", cx).is_some_and(|sub| sub.starts_with("Setup step · running")),
        "{:?}",
        text_of("run-drawer-sub", cx)
    );
    assert!(
        cx.update(|window, _| window.try_find("run-drawer-stop").is_none()),
        "a step is not stopped from here"
    );

    std::fs::write(&go, "").unwrap();
    until(&shell, cx, "the next line follows the end", |cx| {
        shown_lines(&page, cx).is_some_and(|lines| lines == ["first", "second"])
    });
    until(&shell, cx, "the drawer says it is done", |cx| {
        text_of("run-drawer-sub", cx).is_some_and(|sub| sub.starts_with("Setup step · done"))
    });
}

#[gpui_kit::test]
fn a_finished_setup_step_opens_its_output_and_escape_closes_the_drawer(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let go = user.path("go");
    step(&user, "atlas", "short", "echo one\necho two\n");
    // The workspace is still preparing while the first step is read: the setup is listed.
    step(&user, "atlas", "hold", &wait_for(&go));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let page = open_project(&shell, "atlas", cx);
    create(&user, "onboarding");
    until(&shell, cx, "the first step is done", |cx| {
        run_row(&page, "onboarding", StepState::Done, cx).is_some()
            && run_row(&page, "onboarding", StepState::Running, cx).is_some()
    });
    let at = run_row(&page, "onboarding", StepState::Done, cx).unwrap();
    settle(cx);

    cx.update(|window, cx| window.click(format!("setup-step-onboarding-{at}"), cx));
    until(&shell, cx, "the drawer shows its output", |cx| {
        shown_lines(&page, cx).is_some_and(|lines| lines == ["one", "two"])
    });
    settle(cx);
    assert!(
        text_of("run-drawer-sub", cx).is_some_and(|sub| sub.starts_with("Setup step · done")),
        "{:?}",
        text_of("run-drawer-sub", cx)
    );

    cx.simulate_keystrokes("escape");
    settle(cx);
    let tasks = page.read_with(cx, |page, _| page.tasks().clone());
    assert!(cx.update(|window, _| window.try_find("run-drawer-panel").is_none()));
    assert!(tasks.read_with(cx, |t, cx| t.panel().read(cx).run().is_none()));
    std::fs::write(&go, "").unwrap();
}

#[gpui_kit::test]
fn a_failed_setup_step_opens_its_whole_output_not_only_the_tail(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    step(
        &user,
        "atlas",
        "fail",
        "i=1\nwhile [ $i -le 60 ]; do echo \"line $i\"; i=$((i+1)); done\nexit 2\n",
    );
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let page = open_project(&shell, "atlas", cx);
    create(&user, "onboarding");
    until(&shell, cx, "the step failed", |cx| {
        run_row(&page, "onboarding", StepState::Failed, cx).is_some()
    });
    let at = run_row(&page, "onboarding", StepState::Failed, cx).unwrap();
    settle(cx);

    cx.update(|window, cx| window.click(format!("setup-step-onboarding-{at}"), cx));
    let whole: Vec<String> = (1..=60).map(|i| format!("line {i}")).collect();
    until(&shell, cx, "the drawer holds the whole output", |cx| {
        shown_lines(&page, cx).is_some_and(|lines| lines == whole)
    });
    assert!(
        text_of("run-drawer-sub", cx).is_some_and(|sub| sub.starts_with("Setup step · failed")),
        "{:?}",
        text_of("run-drawer-sub", cx)
    );
}

#[gpui_kit::test]
fn a_secret_in_a_setup_step_s_output_is_masked_in_the_drawer(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let go = user.path("go");
    step(
        &user,
        "atlas",
        "secret",
        &format!(
            "echo token ghp_abcdefghijklmnopqrstuvwxyz0123456789\n{}",
            wait_for(&go)
        ),
    );
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let page = open_project(&shell, "atlas", cx);
    create(&user, "onboarding");
    until(&shell, cx, "the step runs", |cx| {
        run_row(&page, "onboarding", StepState::Running, cx).is_some()
    });
    let at = run_row(&page, "onboarding", StepState::Running, cx).unwrap();
    settle(cx);

    cx.update(|window, cx| window.click(format!("setup-step-onboarding-{at}"), cx));
    until(&shell, cx, "the line is shown", |cx| {
        shown_lines(&page, cx).is_some_and(|lines| lines == ["token [masked]"])
    });
}

#[gpui_kit::test]
fn a_setup_step_still_to_do_opens_nothing(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let go = user.path("go");
    step(&user, "atlas", "first", &wait_for(&go));
    step(&user, "atlas", "second", "echo never yet\n");
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let page = open_project(&shell, "atlas", cx);
    create(&user, "onboarding");
    until(&shell, cx, "the first step runs", |cx| {
        run_row(&page, "onboarding", StepState::Running, cx).is_some()
    });
    let at = run_row(&page, "onboarding", StepState::Running, cx).unwrap();
    settle(cx);
    let todo = card_of(&page, "onboarding", cx)
        .unwrap()
        .read_with(cx, |c, _| {
            c.data()
                .steps
                .iter()
                .position(|s| s.state == StepState::Pending)
                .unwrap()
        });
    assert!(todo > at, "the second step waits");

    cx.update(|window, cx| window.click(format!("setup-step-onboarding-{todo}"), cx));
    settle(cx);

    let tasks = page.read_with(cx, |page, _| page.tasks().clone());
    assert!(tasks.read_with(cx, |t, cx| t.panel().read(cx).run().is_none()));
    assert!(cx.update(|window, _| window.try_find("run-drawer-panel").is_none()));
    std::fs::write(&go, "").unwrap();
}

#[gpui_kit::test]
fn the_lines_a_setup_step_writes_unfold_as_they_arrive(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let go = user.path("go");
    step(
        &user,
        "atlas",
        "stream",
        &format!("echo first\n{}echo second\n", wait_for(&go)),
    );
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let page = open_project(&shell, "atlas", cx);
    create(&user, "onboarding");
    until(&shell, cx, "the step runs", |cx| {
        run_row(&page, "onboarding", StepState::Running, cx).is_some()
    });
    let at = run_row(&page, "onboarding", StepState::Running, cx).unwrap();
    settle(cx);
    cx.update(|window, cx| window.click(format!("setup-step-onboarding-{at}"), cx));
    until(&shell, cx, "the first line is shown", |cx| {
        shown_lines(&page, cx).is_some_and(|lines| lines == ["first"])
    });
    settle(cx);

    std::fs::write(&go, "").unwrap();
    until(&shell, cx, "the second line is shown", |cx| {
        shown_lines(&page, cx).is_some_and(|lines| lines.len() == 2)
    });
    let height = |cx: &mut VisualTestContext| {
        cx.update(|window, _| {
            window
                .try_find("command-panel-item-1")
                .map(|item| f32::from(item.bounds().size.height))
        })
    };
    run_for(Duration::from_millis(32), cx);
    let unfolding = height(cx).expect("the new line is drawn in its frame");
    settle(cx);
    let unfolded = height(cx).unwrap();
    assert!(
        unfolding < unfolded - 1.0,
        "the line was part way ({unfolding}) before its full height ({unfolded})"
    );
    // The first line was there when the drawer opened: it did not unfold.
    assert!(cx.update(|window, _| window.try_find("command-panel-item-0").is_none()));
}

/// Adds a setup step written as it is in the project's file.
fn setup(user: &User, project: &str, toml: &str) {
    let file = user.project_file(project);
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str(&format!("\n[[setup]]\n{toml}\n"));
    std::fs::write(file, text).unwrap();
}

fn position_of(
    page: &Entity<ProjectPage>,
    name: &str,
    verb: &str,
    state: StepState,
    cx: &mut VisualTestContext,
) -> Option<usize> {
    card_of(page, name, cx).and_then(|card| {
        card.read_with(cx, |c, _| {
            c.data()
                .steps
                .iter()
                .position(|s| s.verb == verb && s.state == state)
        })
    })
}

#[gpui_kit::test]
fn a_setup_step_that_runs_no_command_opens_nothing_once_it_is_done(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    setup(&user, "atlas", "copy = [\"*.cfg\"]");
    let go = user.path("go");
    step(&user, "atlas", "hold", &wait_for(&go));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let page = open_project(&shell, "atlas", cx);
    create(&user, "onboarding");
    until(&shell, cx, "the copy is done", |cx| {
        position_of(&page, "onboarding", "Copy", StepState::Done, cx).is_some()
    });
    settle(cx);

    let at = position_of(&page, "onboarding", "Copy", StepState::Done, cx).unwrap();
    cx.update(|window, cx| window.click(format!("setup-step-onboarding-{at}"), cx));
    settle(cx);
    let tasks = page.read_with(cx, |page, _| page.tasks().clone());
    assert!(
        tasks.read_with(cx, |t, cx| t.panel().read(cx).run().is_none()),
        "a copy has no output to open"
    );
    assert!(cx.update(|window, _| window.try_find("run-drawer-panel").is_none()));
    std::fs::write(&go, "").unwrap();
}

#[gpui_kit::test]
fn a_failed_setup_step_that_runs_no_command_opens_to_say_why_it_failed(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    setup(&user, "atlas", "link = \"missing.env\"");
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let page = open_project(&shell, "atlas", cx);
    create(&user, "onboarding");
    until(&shell, cx, "the link failed", |cx| {
        position_of(&page, "onboarding", "Link", StepState::Failed, cx).is_some()
    });
    let at = position_of(&page, "onboarding", "Link", StepState::Failed, cx).unwrap();
    settle(cx);

    cx.update(|window, cx| window.click(format!("setup-step-onboarding-{at}"), cx));
    until(&shell, cx, "the drawer says why", |cx| {
        shown_lines(&page, cx).is_some_and(|lines| !lines.is_empty())
    });
    assert!(
        text_of("run-drawer-sub", cx).is_some_and(|sub| sub.starts_with("Setup step · failed")),
        "{:?}",
        text_of("run-drawer-sub", cx)
    );
}
