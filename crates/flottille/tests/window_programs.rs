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
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::programs::ProgramChecks;
use flottille_window::project_settings::{ProjectSettingsPage, Reveal};
use flottille_window::shell::Selection;
use gpui_design_system::omarchy::Environment;
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

/// The remote of `api`, an address on GitHub nothing is ever sent to.
fn remote_of_api(user: &User) {
    user.git(
        &user.path("work/desk/api"),
        &["remote", "add", "origin", "https://github.com/acme/api.git"],
    );
}

const BITBUCKET: &str = "{ kind = \"bitbucket-cloud\", repository = \"acme/api\" }";
const BKT_PAGE: &str = "https://github.com/avivsinai/bitbucket-cli";
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
    cx.update(|cx| flottille_window::init_with(env, cx));
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
    // The row under the link opens its dialog on a click: not on this one.
    assert!(
        page.read_with(cx, |page, _| page.dialog().is_none()),
        "the link opened the tracker's dialog"
    );
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
        Some("gh is not signed in to github.com")
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

    cx.update(|window, cx| window.click("tracker-program-check-again", cx));
    until(&shell, cx, "the program is found", |cx| {
        checks.read_with(cx, |checks, _| checks.tracker().is_some_and(|s| s.found))
    });
    settle(cx);
    assert_eq!(
        label_of("tracker-program-state", cx).as_deref(),
        Some("acli is not signed in to example.atlassian.net")
    );
    assert_eq!(
        label_of("tracker-program-sign-in-0", cx).as_deref(),
        Some("acli jira auth login")
    );
    assert!(
        !has("tracker-program-install", cx),
        "the advice folded away"
    );

    // Signed in, then asked again: nothing is left to say, and the line is gone.
    user.acli_script(json!({}));
    cx.update(|window, cx| window.click("tracker-program-check-again", cx));
    until(&shell, cx, "the program is signed in", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.tracker().is_some_and(|s| s.is_ready())
        })
    });
    settle(cx);
    assert!(!has("tracker-program-state", cx), "the green line is gone");
    assert!(!has("tracker-program-sign-in-0", cx));
    assert!(!has("tracker-program-check-again", cx));
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn the_install_link_of_a_repository_opens_the_page_and_not_the_row_s_dialog(
    cx: &mut TestAppContext,
) {
    // A Bitbucket forge, and no `bkt` on the PATH.
    let user = user_with(&["gh"]);
    desk(&user, Some(BITBUCKET), None);
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let opened = opened_by(&shell, cx);
    let (page, _) = open_settings(&shell, "desk", cx);

    assert_eq!(
        label_of("repository-program-api-install", cx).as_deref(),
        Some(BKT_PAGE)
    );
    cx.update(|window, cx| window.click("repository-program-api-install", cx));
    settle(cx);

    assert_eq!(*opened.lock().unwrap(), [BKT_PAGE]);
    assert!(
        page.read_with(cx, |page, _| page.dialog().is_none()),
        "the link opened the repository's dialog"
    );
}

#[gpui_kit::test]
fn a_changed_state_line_moves_and_comes_to_rest(cx: &mut TestAppContext) {
    // `acli` is missing, then installed and signed out, then signed in.
    let user = user_with(&[]);
    desk(&user, None, Some(JIRA));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (page, checks) = open_settings(&shell, "desk", cx);
    show_tracker(&page, cx);
    assert_eq!(pending_frames(cx), 0, "at rest, nothing moves");

    install(&user, "acli");
    user.acli_script(json!({"signed_in": false}));
    cx.update(|window, cx| window.click("tracker-program-check-again", cx));
    until(&shell, cx, "the program is found", |cx| {
        checks.read_with(cx, |checks, _| checks.tracker().is_some_and(|s| s.found))
    });
    // The red line of the missing program folds away while the signed-out one unfolds, and the
    // ink moves with them: frames are asked for until they have.
    assert!(pending_frames(cx) > 0, "the state line jumped");
    settle(cx);
    assert_eq!(pending_frames(cx), 0, "and it comes to rest");
    assert_eq!(
        label_of("tracker-program-state", cx).as_deref(),
        Some("acli is not signed in to example.atlassian.net")
    );

    user.acli_script(json!({}));
    cx.update(|window, cx| window.click("tracker-program-check-again", cx));
    until(&shell, cx, "the program is signed in", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.tracker().is_some_and(|s| s.is_ready())
        })
    });
    assert!(
        pending_frames(cx) > 0,
        "red to green jumped instead of moving"
    );
    settle(cx);
    assert_eq!(pending_frames(cx), 0);
    assert!(!has("tracker-program-state", cx), "nothing is left to say");
}

#[gpui_kit::test]
fn a_failed_check_again_is_said_and_can_be_asked_again(cx: &mut TestAppContext) {
    let user = user_with(&["gh"]);
    desk(&user, Some(GITHUB), None);
    user.gh_script(json!({"signed_in": false}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (_, checks) = open_settings(&shell, "desk", cx);
    let first = checks.read_with(cx, |checks, _| checks.last_request());
    let good = std::fs::read_to_string(user.project_file("desk")).unwrap();

    // The project's file turns unreadable: the daemon refuses the read.
    std::fs::write(user.project_file("desk"), "not [ toml").unwrap();
    cx.update(|window, cx| window.click("repository-program-api-check-again", cx));
    until(
        &shell,
        cx,
        "a toast says the programs were not checked",
        |cx| {
            shell.read_with(cx, |s, cx| {
                s.toasts()
                    .read(cx)
                    .toasts()
                    .any(|toast| toast.title().contains("not checked"))
            })
        },
    );
    settle(cx);
    assert!(!checks.read_with(cx, |checks, _| checks.is_checking()));
    // What was known stays, and the button asks again.
    assert!(checks.read_with(cx, |checks, _| checks.repository("api").is_some()));
    std::fs::write(user.project_file("desk"), good).unwrap();
    cx.update(|window, cx| window.click("repository-program-api-check-again", cx));
    until(&shell, cx, "the programs are read again", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.last_request() == first + 2 && !checks.is_checking()
        })
    });
    assert!(checks.read_with(cx, |checks, _| checks.repository("api").is_some()));
}

#[gpui_kit::test]
fn saving_the_tracker_reads_the_programs_again(cx: &mut TestAppContext) {
    // Jira with `acli` there; the tracker is then changed to GitHub's issues, which need `gh`.
    let user = user_with(&["acli"]);
    desk(&user, None, Some(JIRA));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (page, checks) = open_settings(&shell, "desk", cx);
    show_tracker(&page, cx);
    let first = checks.read_with(cx, |checks, _| checks.last_request());

    page.update_in(cx, |page, window, cx| page.open_tracker(window, cx));
    settle(cx);
    cx.update(|window, cx| window.click("save-settings-modal", cx));
    until(&shell, cx, "the programs are read after the save", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.last_request() > first && !checks.is_checking()
        })
    });
}

#[gpui_kit::test]
fn the_copy_button_says_copied_in_a_toast(cx: &mut TestAppContext) {
    let user = user_with(&["gh"]);
    desk(&user, Some(GITHUB), None);
    user.gh_script(json!({"signed_in": false}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    open_settings(&shell, "desk", cx);

    cx.update(|window, cx| window.click("repository-program-api-sign-in-0-copy", cx));

    until(&shell, cx, "a toast says Copied", |cx| {
        shell.read_with(cx, |s, cx| {
            s.toasts()
                .read(cx)
                .toasts()
                .any(|toast| toast.title() == "Copied")
        })
    });
    assert_eq!(copied(cx).as_deref(), Some("gh auth login"));
}

/// The window loses the focus and takes it back.
fn leave_and_come_back(cx: &mut VisualTestContext) {
    // A window the test platform opens has the focus once it is asked for.
    cx.update(|window, _| window.activate_window());
    cx.deactivate_window();
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
}

fn toasts_shown(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> usize {
    shell.read_with(cx, |s, cx| s.toasts().read(cx).toasts().count())
}

#[gpui_kit::test]
fn repositories_has_no_check_or_detect_button(cx: &mut TestAppContext) {
    // A repository with a forge and a tracker whose programs are both missing or signed out:
    // everything a button could be for is there, and none is.
    let user = user_with(&["gh"]);
    desk(&user, Some(GITHUB), Some(JIRA));
    user.gh_script(json!({"signed_in": false}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    open_settings(&shell, "desk", cx);

    for id in [
        "check-forge-programs",
        "detect-repositories",
        "check-tracker-program",
    ] {
        assert!(!has(id, cx), "{id} is gone");
    }
    // The only way to ask again is on the line of a program that is not ready.
    assert!(has("repository-program-api-check-again", cx));
    assert!(has("tracker-program-check-again", cx));
}

#[gpui_kit::test]
fn the_window_regaining_focus_reads_the_programs_again(cx: &mut TestAppContext) {
    // `gh` is signed out; it is signed in while the page stays open and the window is away.
    let user = user_with(&["gh"]);
    desk(&user, Some(GITHUB), None);
    remote_of_api(&user);
    user.gh_script(json!({"signed_in": false}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (_, checks) = open_settings(&shell, "desk", cx);
    assert_eq!(
        label_of("repository-program-api-state", cx).as_deref(),
        Some("gh is not signed in to github.com")
    );
    let first = checks.read_with(cx, |checks, _| checks.last_request());
    user.gh_script(json!({}));

    // Losing the focus reads nothing; taking it back does.
    cx.deactivate_window();
    cx.run_until_parked();
    assert_eq!(
        checks.read_with(cx, |checks, _| checks.last_request()),
        first
    );
    cx.update(|window, _| window.activate_window());
    until(&shell, cx, "the program is signed in", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.repository("api").is_some_and(|s| s.is_ready())
        })
    });
    settle(cx);

    assert!(checks.read_with(cx, |checks, _| checks.last_request()) > first);
    assert_eq!(
        label_of("repository-forge-api", cx).as_deref(),
        Some("GitHub · gh, ready")
    );
    assert!(!has("repository-program-api-state", cx));
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn the_focus_taken_back_on_another_tab_reads_nothing_until_the_repositories_show(
    cx: &mut TestAppContext,
) {
    let user = user_with(&["gh"]);
    desk(&user, Some(GITHUB), None);
    user.gh_script(json!({"signed_in": false}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (page, checks) = open_settings(&shell, "desk", cx);
    let repositories = page.read_with(cx, |page, _| page.repositories().clone());
    page.update(cx, |page, cx| page.choose(1, cx));
    settle(cx);
    let (first, listed) = (
        checks.read_with(cx, |checks, _| checks.last_request()),
        repositories.read_with(cx, |section, _| section.last_request()),
    );
    user.gh_script(json!({}));

    leave_and_come_back(cx);
    settle(cx);
    assert_eq!(
        checks.read_with(cx, |checks, _| checks.last_request()),
        first
    );
    assert_eq!(
        repositories.read_with(cx, |section, _| section.last_request()),
        listed
    );

    // The tab shown again reads both.
    page.update(cx, |page, cx| page.choose(0, cx));
    until(&shell, cx, "the program is signed in", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.repository("api").is_some_and(|s| s.is_ready())
        })
    });
    assert!(repositories.read_with(cx, |section, _| section.last_request()) > listed);
}

#[gpui_kit::test]
fn a_read_on_focus_shows_no_toast(cx: &mut TestAppContext) {
    let user = user_with(&["gh"]);
    desk(&user, Some(GITHUB), None);
    user.gh_script(json!({"signed_in": false}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (_, checks) = open_settings(&shell, "desk", cx);
    assert_eq!(toasts_shown(&shell, cx), 0);

    // A read that finds something new says nothing in a toast: the line moves.
    user.gh_script(json!({}));
    leave_and_come_back(cx);
    until(&shell, cx, "the program is signed in", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.repository("api").is_some_and(|s| s.is_ready())
        })
    });
    settle(cx);
    assert_eq!(toasts_shown(&shell, cx), 0);

    // Nor one that fails: what was known stays.
    let before = checks.read_with(cx, |checks, _| checks.last_request());
    std::fs::write(user.project_file("desk"), "not [ toml").unwrap();
    leave_and_come_back(cx);
    until(&shell, cx, "the read ended", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.last_request() > before && !checks.is_checking()
        })
    });
    settle(cx);
    assert_eq!(toasts_shown(&shell, cx), 0);
    assert!(checks.read_with(cx, |checks, _| checks.repository("api").is_some()));
}

#[gpui_kit::test]
fn check_again_is_only_on_a_program_not_ready(cx: &mut TestAppContext) {
    // `gh` signed out for the repository, `acli` ready for the tracker.
    let user = user_with(&["gh", "acli"]);
    desk(&user, Some(GITHUB), Some(JIRA));
    user.gh_script(json!({"signed_in": false}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (_, checks) = open_settings(&shell, "desk", cx);

    assert!(has("repository-program-api-check-again", cx));
    assert!(
        !has("tracker-program-check-again", cx),
        "the tracker is ready"
    );

    // Asked on its line, once `gh` is signed in, the line and its button go.
    user.gh_script(json!({}));
    cx.update(|window, cx| window.click("repository-program-api-check-again", cx));
    until(&shell, cx, "the program is signed in", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.repository("api").is_some_and(|s| s.is_ready())
        })
    });
    settle(cx);
    assert!(!has("repository-program-api-check-again", cx));
    assert!(!has("tracker-program-check-again", cx));
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn the_forge_and_its_program_are_one_value(cx: &mut TestAppContext) {
    let user = user_with(&["gh", "acli"]);
    desk(&user, Some(GITHUB), Some(JIRA));
    // The remote leads to the forge: the repository on it is not said twice.
    remote_of_api(&user);
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (_, _) = open_settings(&shell, "desk", cx);

    // One value: the forge, its program, and the program's state; no line of its own.
    assert_eq!(
        label_of("repository-forge-api", cx).as_deref(),
        Some("GitHub · gh, ready")
    );
    assert!(
        !has("repository-program-api-state", cx),
        "the green line is gone"
    );
    // The tracker says it the same way.
    assert_eq!(
        label_of("tracker-program-value", cx).as_deref(),
        Some("acli, ready")
    );
    assert!(!has("tracker-program-state", cx));
    // The version and the host it is signed in to are in the tooltip.
    cx.update(|window, cx| window.hover("repository-forge-api", cx));
    settle(cx);
    let tip = label_of("repository-forge-api-tooltip", cx).unwrap();
    assert!(
        tip.contains("gh 2.60.0") && tip.contains("github.com"),
        "{tip}"
    );
}

#[gpui_kit::test]
fn a_forge_whose_program_is_not_ready_has_no_check_mark(cx: &mut TestAppContext) {
    let user = user_with(&["gh"]);
    desk(&user, Some(GITHUB), None);
    remote_of_api(&user);
    user.gh_script(json!({"signed_in": false}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (_, checks) = open_settings(&shell, "desk", cx);
    assert_eq!(
        label_of("repository-forge-api", cx).as_deref(),
        Some("GitHub · gh")
    );

    // Signed in: the mark comes in, moving, and comes to rest.
    user.gh_script(json!({}));
    cx.update(|window, cx| window.click("repository-program-api-check-again", cx));
    until(&shell, cx, "the program is signed in", |cx| {
        checks.read_with(cx, |checks, _| {
            checks.repository("api").is_some_and(|s| s.is_ready())
        })
    });
    assert!(pending_frames(cx) > 0, "the mark jumped in");
    settle(cx);
    assert_eq!(pending_frames(cx), 0);
    assert_eq!(
        label_of("repository-forge-api", cx).as_deref(),
        Some("GitHub · gh, ready")
    );
}

#[gpui_kit::test]
fn bkt_signed_out_gives_both_sign_in_commands_with_a_copy_button_each(cx: &mut TestAppContext) {
    let user = user_with(&["bkt"]);
    desk(&user, Some(BITBUCKET), None);
    user.bkt_script(json!({"contexts": []}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    open_settings(&shell, "desk", cx);

    let first = "bkt auth login https://bitbucket.org --kind cloud --web";
    let second = "bkt context create bitbucket-org-acme --host api.bitbucket.org --workspace acme";
    assert_eq!(
        label_of("repository-program-api-sign-in-0", cx).as_deref(),
        Some(first)
    );
    assert_eq!(
        label_of("repository-program-api-sign-in-1", cx).as_deref(),
        Some(second)
    );
    for (at, command) in [(0, first), (1, second)] {
        cx.update(|window, cx| {
            window.click(format!("repository-program-api-sign-in-{at}-copy"), cx)
        });
        settle(cx);
        assert_eq!(copied(cx).as_deref(), Some(command));
    }
}

#[gpui_kit::test]
fn acli_signed_in_to_another_site_says_so_with_the_switch_command(cx: &mut TestAppContext) {
    let user = user_with(&["acli"]);
    desk(&user, None, Some(JIRA));
    user.acli_script(json!({"site": "other.atlassian.net"}));
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    let (page, _) = open_settings(&shell, "desk", cx);
    show_tracker(&page, cx);

    assert_eq!(
        label_of("tracker-program-state", cx).as_deref(),
        Some("acli is signed in to other.atlassian.net, not to example.atlassian.net")
    );
    assert_eq!(
        label_of("tracker-program-sign-in-0", cx).as_deref(),
        Some("acli jira auth switch --site example.atlassian.net")
    );
}
