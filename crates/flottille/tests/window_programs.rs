//! What the project's Settings say of the command lines its forges and tracker need (#306): `gh`,
//! `bkt` and `acli`, found or not with their version, signed in or not, how to install one that
//! is missing and how to sign in to one that is signed out, and Check again. On GPUI's test
//! platform in front of a real daemon, with fakes of the three programs on a PATH of the test's
//! own, throwaway repositories and temporary XDG folders. No window opens on a screen, no real
//! `gh`, `bkt` nor `acli` is run, none of their folders is read.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::programs::ProgramChecks;
use flottille_window::project_settings::{ProjectSettingsPage, Reveal};
use flottille_window::shell::Selection;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

const ACLI_PAGE: &str = "https://developer.atlassian.com/cloud/acli/guides/install-acli/";

/// A user whose PATH is `bin`: `git`, and the fakes among `gh`, `bkt` and `acli` named. A program
/// not named is not there, the machine's own included.
fn user_with(programs: &[&str]) -> User {
    let user = User::with_fake_agent();
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    std::fs::create_dir_all(user.path("bin")).unwrap();
    std::os::unix::fs::symlink(common::program("git"), user.path("bin/git")).unwrap();
    for program in ["gh", "bkt", "acli"] {
        std::fs::create_dir_all(user.path(program)).unwrap();
        if programs.contains(&program) {
            install(&user, program);
        }
    }
    let (path, gh, bkt, acli) = (
        user.path("bin"),
        user.path("gh"),
        user.path("bkt"),
        user.path("acli"),
    );
    user.with_env("PATH", path.to_str().unwrap())
        .with_env("FLOTTILLE_FAKE_GH_DIR", gh.to_str().unwrap())
        .with_env("FLOTTILLE_FAKE_BKT_DIR", bkt.to_str().unwrap())
        .with_env("FLOTTILLE_FAKE_ACLI_DIR", acli.to_str().unwrap())
}

/// The fake `program` put on the user's PATH.
fn install(user: &User, program: &str) {
    let fake = match program {
        "gh" => common::fake_gh(),
        "bkt" => common::fake_bkt(),
        _ => common::fake_acli(),
    };
    std::os::unix::fs::symlink(fake, user.path(&format!("bin/{program}"))).unwrap();
}

/// A project `desk` whose repository `api` has the forge and whose tracker is the one given (the
/// text of the file after the repository).
fn desk(user: &User, forge: Option<&str>, tracker: Option<&str>) {
    let folder = user.path("work/desk");
    user.git_init(&folder.join("api"));
    std::fs::write(folder.join("api/README.md"), "api\n").unwrap();
    user.git(&folder.join("api"), &["add", "."]);
    user.git(&folder.join("api"), &["commit", "--quiet", "-m", "start"]);
    let forge = forge.map_or(String::new(), |forge| format!("forge = {forge}\n"));
    user.write_project_file(
        "desk",
        &format!(
            "folder = \"{}\"\n\n[[repositories]]\npath = \"api\"\n{forge}\n{}",
            folder.display(),
            tracker.unwrap_or_default()
        ),
    );
}

const GITHUB: &str = "{ kind = \"github\", repository = \"acme/api\" }";
const JIRA: &str =
    "[tracker]\nkind = \"jira\"\nsite = \"example.atlassian.net\"\nprojects = [\"ABC\"]\n";

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
    cx.simulate_resize(size(px(1280.), px(2400.)));
    (shell, cx)
}

/// Runs the window until `check` holds, as real frames would.
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

fn pending_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

/// Opens the project's settings and waits for the programs to be checked once.
fn open_settings(
    shell: &Entity<Shell>,
    project: &str,
    cx: &mut VisualTestContext,
) -> (Entity<ProjectSettingsPage>, Entity<ProgramChecks>) {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings(project.into()), window, cx)
    });
    let page = |cx: &mut VisualTestContext| -> Option<Entity<ProjectSettingsPage>> {
        shell.read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
    };
    until(shell, cx, "the settings are read", |cx| {
        page(cx).is_some_and(|page| page.read_with(cx, |page, cx| page.has_read(cx)))
    });
    let page = page(cx).unwrap();
    let checks = page.read_with(cx, |page, _| page.programs().clone());
    until(shell, cx, "the programs are checked", |cx| {
        checks.read_with(cx, |checks, _| checks.checked().is_some())
    });
    settle(cx);
    (page, checks)
}

/// The Tracker tab of the settings.
fn show_tracker(page: &Entity<ProjectSettingsPage>, cx: &mut VisualTestContext) {
    page.update(cx, |page, cx| page.reveal(Reveal::Tracker, cx));
    settle(cx);
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

fn copied(cx: &mut VisualTestContext) -> Option<String> {
    cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

/// The links the window opened.
fn opened_by(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Arc<Mutex<Vec<String>>> {
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    shell.update(cx, |shell, _| {
        shell.set_opener(Arc::new(move |path: &Path| {
            seen.lock().unwrap().push(path.display().to_string());
        }))
    });
    opened
}

#[gpui_kit::test]
fn a_tracker_whose_program_is_missing_says_how_to_install_it(cx: &mut TestAppContext) {
    // Jira, and no `acli` on the PATH.
    let user = user_with(&["gh"]);
    desk(&user, None, Some(JIRA));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let opened = opened_by(&shell, cx);

    let (page, checks) = open_settings(&shell, "desk", cx);
    show_tracker(&page, cx);

    let state = checks
        .read_with(cx, |checks, _| checks.tracker().cloned())
        .unwrap();
    assert!(!state.found, "{state:?}");
    assert_eq!(state.program, "acli");
    assert_eq!(
        label_of("tracker-program-state", cx).as_deref(),
        Some("acli not found")
    );
    // The program's own page, a link the system opens.
    assert_eq!(
        label_of("tracker-program-install", cx).as_deref(),
        Some(ACLI_PAGE)
    );
    cx.update(|window, cx| window.click("tracker-program-install", cx));
    assert_eq!(*opened.lock().unwrap(), [ACLI_PAGE]);
    settle(cx);
    // No command fits most systems for `acli`: none is offered, and nothing to sign in to yet.
    assert!(!has("tracker-program-install-command", cx));
    assert!(!has("tracker-program-sign-in-0", cx));
    // Flottille installs nothing: nothing ran.
    assert!(user.acli_received().is_empty());
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn a_forge_whose_program_is_signed_out_gives_the_sign_in_command(cx: &mut TestAppContext) {
    let user = user_with(&["gh"]);
    desk(&user, Some(GITHUB), None);
    user.gh_script(json!({"signed_in": false}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);

    let (_, checks) = open_settings(&shell, "desk", cx);

    let state = checks
        .read_with(cx, |checks, _| checks.repository("api").cloned())
        .unwrap();
    assert!(state.found && state.signed_in == Some(false), "{state:?}");
    assert_eq!(
        label_of("repository-program-api-state", cx).as_deref(),
        Some("gh 2.60.0 · not signed in to github.com")
    );
    assert_eq!(
        label_of("repository-program-api-sign-in-0", cx).as_deref(),
        Some("gh auth login")
    );
    assert!(
        !has("repository-program-api-sign-in-1", cx),
        "one command signs in to GitHub"
    );
    assert!(
        !has("repository-program-api-install", cx),
        "it is installed"
    );
    // The button copies the command, and a toast says so.
    cx.update(|window, cx| window.click("repository-program-api-sign-in-0-copy", cx));
    settle(cx);
    assert_eq!(copied(cx).as_deref(), Some("gh auth login"));
    // What the program printed of the account is never shown.
    assert!(
        label_of("repository-program-api-state", cx)
            .unwrap()
            .find("someone")
            .is_none()
    );
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn check_again_reads_the_program_s_state_anew(cx: &mut TestAppContext) {
    // Jira, `acli` missing: it is installed, then signed out, while the page stays open.
    let user = user_with(&[]);
    desk(&user, None, Some(JIRA));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (page, checks) = open_settings(&shell, "desk", cx);
    show_tracker(&page, cx);
    assert_eq!(
        label_of("tracker-program-state", cx).as_deref(),
        Some("acli not found")
    );
    let first = checks.read_with(cx, |checks, _| checks.last_request());

    // Nothing reads by itself, however long the page stays open.
    install(&user, "acli");
    user.acli_script(json!({"signed_in": false}));
    for _ in 0..5 {
        cx.executor().advance_clock(Duration::from_secs(30));
        cx.run_until_parked();
    }
    settle(cx);
    assert_eq!(
        checks.read_with(cx, |checks, _| checks.last_request()),
        first,
        "no loop"
    );
    assert_eq!(
        label_of("tracker-program-state", cx).as_deref(),
        Some("acli not found")
    );
    assert!(user.acli_received().is_empty());

    cx.update(|window, cx| window.click("check-tracker-program", cx));
    until(&shell, cx, "the program is found", |cx| {
        checks.read_with(cx, |checks, _| checks.tracker().is_some_and(|s| s.found))
    });
    settle(cx);
    assert_eq!(
        label_of("tracker-program-state", cx).as_deref(),
        Some("acli 1.3.2-stable · not signed in to example.atlassian.net")
    );
    assert_eq!(
        label_of("tracker-program-sign-in-0", cx).as_deref(),
        Some("acli jira auth login")
    );
    assert!(
        !has("tracker-program-install", cx),
        "the advice folded away"
    );

    // Signed in, then asked again.
    user.acli_script(json!({}));
    cx.update(|window, cx| window.click("check-tracker-program", cx));
    until(&shell, cx, "the program is signed in", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.tracker().is_some_and(|s| s.is_ready())
        })
    });
    settle(cx);
    assert_eq!(
        label_of("tracker-program-state", cx).as_deref(),
        Some("acli 1.3.2-stable · signed in to example.atlassian.net")
    );
    assert!(!has("tracker-program-sign-in-0", cx));
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}
