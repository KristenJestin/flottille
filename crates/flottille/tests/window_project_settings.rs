//! The project's Settings in five tabs (#703, `docs/design.md` §10, `docs/reference/gpui.md` rules
//! 13 and 14): Repositories (with the tracker), Workspaces, Tasks, Instructions and Features, the
//! Features tab a list that opens a page for each project-scoped feature. On GPUI's test platform
//! in front of a real daemon in temporary folders. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::project_settings::{ProjectSettingsPage, Reveal};
use flottille_window::settings::SettingsPage;
use flottille_window::shell::Selection;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

/// The tabs, in order.
const TABS: [&str; 5] = [
    "repositories",
    "workspaces",
    "tasks",
    "instructions",
    "features",
];

/// The tabs of the project's settings before the redesign that are no tab now.
const FORMER: [&str; 5] = ["tracker", "setup", "skills", "commands", "rules"];

fn user() -> User {
    let user = User::with_fake_providers();
    user.provider_script(
        "claude",
        json!({"models": ["fake-small", "fake-large"], "efforts": ["low", "high"]}),
    );
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

/// A project over a throwaway repository.
fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_window::init_with(env, cx));
    let connect: flottille_window::link::Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(2400.)));
    (shell, cx)
}

fn until(
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        shell.update_in(cx, |shell, window, cx| shell.poll(window, cx));
        cx.executor().advance_clock(Duration::from_millis(50));
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

fn visible(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.update(|window, _| window.try_find(id.to_owned()).is_some_and(|f| f.visible()))
}

fn label(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn top(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| f32::from(window.find(id).bounds().origin.y))
}

fn left(cx: &mut VisualTestContext, id: &str) -> f32 {
    let id = id.to_owned();
    cx.update(|window, _| f32::from(window.find(id).bounds().origin.x))
}

/// How far an element that carries its fade says it is (0 to 1).
fn faded_by(cx: &mut VisualTestContext, id: &str) -> Option<f32> {
    cx.update(|window, _| {
        window
            .try_find(id.to_owned())
            .and_then(|found| found.value().and_then(|value| value.parse().ok()))
    })
}

/// The window's clock and frames going on for `by`, with no wait for the daemon.
fn run_for(by: Duration, cx: &mut VisualTestContext) {
    let mut gone = Duration::ZERO;
    while gone < by {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += Duration::from_millis(16);
    }
}

fn click(cx: &mut VisualTestContext, id: &str) {
    let id = id.to_owned();
    cx.update(|window, cx| window.click(id, cx));
    settle(cx);
}

/// The project's settings, once the daemon answered each of their sections.
fn open_settings(
    shell: &Entity<Shell>,
    project: &str,
    cx: &mut VisualTestContext,
) -> Entity<ProjectSettingsPage> {
    shell.update_in(cx, |s, window, cx| {
        s.open(Selection::ProjectSettings(project.into()), window, cx)
    });
    until(shell, cx, "the project's settings are read", |cx| {
        shell
            .read_with(cx, |s, cx| {
                s.main().read(cx).project_settings_page().cloned()
            })
            .is_some_and(|page| {
                page.read_with(cx, |page, cx| {
                    page.name() == project && page.is_loaded() && page.has_read(cx)
                })
            })
    });
    settle(cx);
    shell
        .read_with(cx, |s, cx| {
            s.main().read(cx).project_settings_page().cloned()
        })
        .unwrap()
}

fn tab(cx: &mut VisualTestContext, key: &str) {
    click(cx, &format!("project-settings-tabs-{key}"));
}

fn turn(user: &User, name: &str, enabled: bool, project: Option<&str>) {
    user.call(
        "extension.set",
        json!({"name": name, "enabled": enabled, "project": project}),
    )
    .unwrap();
}

/// What `extension.list` says of `name`.
fn listed(user: &User, name: &str) -> Value {
    user.call("extension.list", json!({})).unwrap()["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .cloned()
        .unwrap()
}

#[gpui_kit::test]
fn project_settings_have_five_tabs_and_open_on_repositories(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, "app", cx);

    for key in TABS {
        assert!(
            visible(cx, &format!("project-settings-tabs-{key}")),
            "{key}"
        );
    }
    for key in FORMER {
        assert!(
            !present(cx, &format!("project-settings-tabs-{key}")),
            "{key} is no tab now"
        );
    }
    assert!(!present(cx, "project-settings-tabs-specs"));
    let xs: Vec<f32> = TABS
        .iter()
        .map(|key| left(cx, &format!("project-settings-tabs-{key}")))
        .collect();
    assert!(
        xs.windows(2).all(|pair| pair[0] < pair[1]),
        "the tabs are in their order, left to right: {xs:?}"
    );
    cx.update(|window, _| {
        assert_eq!(
            window.find("project-settings-tabs-repositories").selected(),
            Some(true)
        );
    });
    assert!(visible(cx, "repositories-title"));
    assert!(!present(cx, "setup-title"), "one tab is shown at a time");

    // Each tab shows what it holds alone.
    for (key, first) in [
        ("workspaces", "setup-title"),
        ("tasks", "tasks-title"),
        ("instructions", "rules-title"),
        ("features", "project-features-list"),
    ] {
        tab(cx, key);
        assert!(visible(cx, first), "{key} shows {first}");
        assert!(!present(cx, "repositories-title"), "{key} is shown alone");
    }
}

#[gpui_kit::test]
fn the_tracker_is_a_section_of_repositories(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, "app", cx);

    assert!(
        visible(cx, "tracker-title"),
        "Tracker is under Repositories"
    );
    assert!(top(cx, "tracker-title") > top(cx, "repositories-title"));
    // Its row says where the tickets come from, `Nowhere` for a project with none.
    assert!(
        label(cx, "tracker-row").is_some_and(|row| row.contains("Nowhere")),
        "{:?}",
        label(cx, "tracker-row")
    );
    assert_eq!(
        label(cx, "tracker-tickets-from-label").as_deref(),
        Some("Tickets from")
    );
}

#[gpui_kit::test]
fn the_folder_offers_as_in_settings_with_the_folder_it_gives(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, "app", cx);
    tab(cx, "workspaces");
    let folder = page.read_with(cx, |page, _| page.workspaces_folder().clone());
    until(&shell, cx, "the folder is read", |cx| {
        folder.read_with(cx, |folder, _| folder.folder().is_some())
    });
    settle(cx);

    assert_eq!(
        label(cx, "workspaces-section-app-title").as_deref(),
        Some("Folder")
    );
    assert!(
        !present(cx, "workspaces-section-app-sentence"),
        "the folder has no sentence"
    );
    assert_eq!(label(cx, "workspaces-choice-app").as_deref(), Some("Where"));
    assert_eq!(
        label(cx, "workspaces-choice-app-data").as_deref(),
        Some("As in Settings")
    );
    // Its line is the folder it gives, relative to `~`.
    let line = label(cx, "workspaces-choice-app-data-line").unwrap();
    // The folder the default gives for this project, here under the data folder.
    assert!(line.ends_with("workspaces/app"), "{line}");
    assert_eq!(
        label(cx, "workspaces-choice-app-own").as_deref(),
        Some("A folder for this project")
    );

    // The field unfolds under the second choice only.
    assert!(!visible(cx, "workspaces-path-app"));
    click(cx, "workspaces-choice-app-own");
    assert!(visible(cx, "workspaces-path-app"));
    // No Save at rest: it unfolds once what is typed differs.
    assert!(
        !present(cx, "workspaces-save-app"),
        "Save only once changed"
    );
    click(cx, "workspaces-choice-app-data");
    assert!(!visible(cx, "workspaces-path-app"), "folded away again");
}

#[gpui_kit::test]
fn a_step_s_time_limit_is_a_duration(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, "app", cx);
    tab(cx, "workspaces");

    click(cx, "add-setup");
    click(cx, "setup-kind");
    assert!(visible(cx, "setup-kind"), "the form is open");
    click(cx, "setup-kind-run");
    assert!(
        present(cx, "setup-value-field-label"),
        "a run has its command line"
    );
    assert!(present(cx, "setup-limit-field-label"), "and a time limit");
    assert!(
        present(cx, "setup-limit"),
        "the limit is a select, not a field of seconds"
    );
    assert_eq!(
        label(cx, "setup-limit-field-label").as_deref(),
        Some("Time limit")
    );
    // Its list fades in, durations in words.
    click(cx, "setup-limit");
    assert_eq!(label(cx, "setup-limit-600").as_deref(), Some("10 min"));
    click(cx, "setup-limit-600");

    let value = shell.read_with(cx, |s, cx| {
        s.main()
            .read(cx)
            .project_settings_page()
            .unwrap()
            .read(cx)
            .setup()
            .read(cx)
            .value_field()
            .unwrap()
            .clone()
    });
    cx.update(|window, cx| value.update(cx, |f, cx| f.set_value("npm ci", window, cx)));
    click(cx, "save-setup");
    until(&shell, cx, "the step is listed", |cx| {
        present(cx, "setup-step-1")
    });
    settle(cx);
    let file = std::fs::read_to_string(user.project_file("app")).unwrap();
    assert!(file.contains("time_limit = 600"), "{file}");
    // The row says it in words, and where it is done.
    assert_eq!(
        label(cx, "setup-step-1-line").as_deref(),
        Some("in the project folder · stops after 10 min")
    );
}

#[gpui_kit::test]
fn commands_and_skills_are_sections_of_instructions(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, "app", cx);
    tab(cx, "instructions");

    // In order: Rules, Commands, Skills, the checks, the pull request template.
    let titles = [
        "rules-title",
        "commands-title",
        "skills-title",
        "message-checks-app-title",
        "pull-request-template-section-app-title",
    ];
    let mut at = f32::MIN;
    for id in titles {
        assert!(visible(cx, id), "{id}");
        let y = top(cx, id);
        assert!(y > at, "{id} is under the one before it");
        at = y;
    }
    assert_eq!(
        label(cx, "message-checks-app-title").as_deref(),
        Some("Commit and pull request checks")
    );
    assert_eq!(
        label(cx, "message-checks-field-app-label").as_deref(),
        Some("One command line per line")
    );
    // The fold of the rules every project has, and the way to edit them.
    assert!(present(cx, "rules-global-toggle-app"));
    assert_eq!(
        label(cx, "rules-global-toggle-app").as_deref(),
        Some("Rules for every project, read only")
    );
    click(cx, "rules-global-toggle-app");
    assert_eq!(
        label(cx, "rules-global-edit-app").as_deref(),
        Some("Edit in Settings")
    );
    // New skill, as every gesture of adding is New.
    assert_eq!(label(cx, "add-skill").as_deref(), Some("New skill"));
    assert_eq!(label(cx, "add-command").as_deref(), Some("New command"));
}

/// The project's Features tab, with Specs on globally so that its row opens a page.
fn open_features(
    user: &User,
    shell: &Entity<Shell>,
    cx: &mut VisualTestContext,
) -> Entity<ProjectSettingsPage> {
    let page = open_settings(shell, "app", cx);
    tab(cx, "features");
    until(shell, cx, "the features are listed", |cx| {
        present(cx, "project-features-specs") || present(cx, "project-features-review")
    });
    let _ = user;
    page
}

#[gpui_kit::test]
fn a_project_feature_page_offers_as_in_settings_first(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    turn(&user, "specs", true, None);
    let (shell, cx) = window(&user, cx);
    open_features(&user, &shell, cx);
    until(&shell, cx, "Specs is a row", |cx| {
        visible(cx, "project-features-specs")
    });
    click(cx, "project-features-specs");
    until(&shell, cx, "its page is in", |cx| {
        visible(cx, "project-specs-switch")
    });
    settle(cx);

    assert_eq!(
        label(cx, "project-features-path").as_deref(),
        Some("Features / Specs")
    );
    // The global switch's label, its hint says it is on everywhere until turned off here.
    assert_eq!(
        label(cx, "project-specs-switch-label").as_deref(),
        Some("Write specs before building")
    );
    assert_eq!(
        label(cx, "project-specs-switch-hint").as_deref(),
        Some("On in every project unless you turn it off here.")
    );
    assert!(
        label(cx, "project-specs-usage").is_some_and(|usage| usage.contains("Write the spec")),
        "the same usage line"
    );
    // Its value offers As in Settings first.
    assert_eq!(
        label(cx, "project-specs-setting-language-label").as_deref(),
        Some("Written in")
    );
    click(cx, "project-specs-setting-language-select");
    assert!(
        label(cx, "project-specs-setting-language-select-default")
            .is_some_and(|first| first.starts_with("As in Settings")),
        "{:?}",
        label(cx, "project-specs-setting-language-select-default")
    );
    assert!(
        !present(cx, "project-specs-setting-language-set"),
        "nothing is set here yet"
    );
}

#[gpui_kit::test]
fn a_value_set_in_the_project_says_set_here_and_can_go_back(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    turn(&user, "specs", true, None);
    let (shell, cx) = window(&user, cx);
    open_features(&user, &shell, cx);
    until(&shell, cx, "Specs is a row", |cx| {
        visible(cx, "project-features-specs")
    });
    click(cx, "project-features-specs");
    until(&shell, cx, "its page is in", |cx| {
        visible(cx, "project-specs-setting-language-select")
    });

    click(cx, "project-specs-setting-language-select");
    click(cx, "project-specs-setting-language-select-fr");
    until(&shell, cx, "the project has its own language", |_| {
        listed(&user, "specs")["projects"][0]["settings"]["language"] == "fr"
    });
    until(&shell, cx, "the dot is shown", |cx| {
        settle(cx);
        visible(cx, "project-specs-setting-language-set")
    });
    assert_eq!(
        label(cx, "project-specs-setting-language-set").as_deref(),
        Some("set here")
    );
    assert_eq!(
        label(cx, "project-specs-setting-language-reset").as_deref(),
        Some("Use Settings' value")
    );

    click(cx, "project-specs-setting-language-reset");
    until(
        &shell,
        cx,
        "the project goes back to Settings' value",
        |_| {
            listed(&user, "specs")["projects"][0]["settings"]
                .get("language")
                .is_none_or(Value::is_null)
        },
    );
    until(&shell, cx, "the dot goes", |cx| {
        settle(cx);
        !present(cx, "project-specs-setting-language-set")
            && !present(cx, "project-specs-setting-language-reset")
    });
}

#[gpui_kit::test]
fn a_project_overrides_the_default_mode_and_goes_back_to_settings(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    turn(&user, "specs", true, None);
    let (shell, cx) = window(&user, cx);
    open_features(&user, &shell, cx);
    until(&shell, cx, "Specs is a row", |cx| {
        visible(cx, "project-features-specs")
    });
    click(cx, "project-features-specs");
    until(&shell, cx, "its radios are in", |cx| {
        visible(cx, "project-specs-setting-mode-default")
    });
    settle(cx);

    // As in Settings is the first radio, and says what Settings gives.
    assert_eq!(
        label(cx, "project-specs-setting-mode-default").as_deref(),
        Some("As in Settings (Kept in Flottille)")
    );
    let chosen = |cx: &mut VisualTestContext, id: &str| {
        cx.update(|window, _| window.find(id.to_owned()).selected() == Some(true))
    };
    assert!(chosen(cx, "project-specs-setting-mode-default"));
    assert!(!present(cx, "project-specs-setting-mode-set"));

    click(cx, "project-specs-setting-mode-ticket");
    until(&shell, cx, "the project has its own default", |_| {
        listed(&user, "specs")["projects"][0]["settings"]["mode"] == "ticket"
    });
    until(&shell, cx, "the dot is shown", |cx| {
        settle(cx);
        visible(cx, "project-specs-setting-mode-set")
    });
    assert!(chosen(cx, "project-specs-setting-mode-ticket"));

    click(cx, "project-specs-setting-mode-reset");
    until(&shell, cx, "the project goes back to Settings", |_| {
        listed(&user, "specs")["projects"][0]["settings"]
            .get("mode")
            .is_none_or(Value::is_null)
    });
    until(
        &shell,
        cx,
        "the dot goes and the first radio is chosen",
        |cx| {
            settle(cx);
            !present(cx, "project-specs-setting-mode-set")
                && chosen(cx, "project-specs-setting-mode-default")
        },
    );
}

#[gpui_kit::test]
fn a_feature_off_globally_is_a_faded_row_with_a_link(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    turn(&user, "review", true, None);
    // Specs stays off in Settings.
    let (shell, cx) = window(&user, cx);
    open_features(&user, &shell, cx);
    until(&shell, cx, "both rows are drawn", |cx| {
        visible(cx, "project-features-specs") && visible(cx, "project-features-review")
    });

    assert_eq!(
        label(cx, "project-features-specs-off").as_deref(),
        Some("Off in Settings")
    );
    assert_eq!(
        faded_by(cx, "project-features-specs-off"),
        Some(1.0),
        "the row is faded"
    );
    assert!(
        !present(cx, "project-features-review-off"),
        "a feature on in Settings is no faded row"
    );
    // In the order the daemon lists them, not under the others.
    let order: Vec<String> = user.call("extension.list", json!({})).unwrap()["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .filter(|name| ["review", "specs"].contains(name))
        .map(str::to_owned)
        .collect();
    assert_eq!(order.len(), 2);
    assert!(
        top(cx, &format!("project-features-{}", order[0]))
            < top(cx, &format!("project-features-{}", order[1])),
        "in their declared order: {order:?}"
    );
    // It opens nothing of the project.
    click(cx, "project-features-specs");
    assert!(
        !present(cx, "project-features-path"),
        "no page of the project"
    );
    // The faded row opens nothing of the project: its link opens the feature's page in Settings.
    click(cx, "project-features-specs-off");
    until(&shell, cx, "Settings opens on the page", |cx| {
        shell
            .read_with(cx, |s, cx| s.main().read(cx).settings_page().cloned())
            .is_some_and(|page: Entity<SettingsPage>| {
                page.read_with(cx, |page, _| page.open_feature() == Some("specs"))
            })
    });
}

#[gpui_kit::test]
fn the_global_page_names_a_project_that_turns_it_off(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "notes-app");
    project(&user, "shop");
    turn(&user, "specs", true, None);
    let (shell, cx) = window(&user, cx);
    shell.update_in(cx, |s, window, cx| s.open(Selection::Settings, window, cx));
    until(&shell, cx, "the features are listed", |cx| {
        present(cx, "settings-tabs-features")
    });
    click(cx, "settings-tabs-features");
    until(&shell, cx, "Specs is a row", |cx| {
        present(cx, "features-specs")
    });
    click(cx, "features-specs");
    until(&shell, cx, "its page is in", |cx| {
        visible(cx, "specs-switch")
    });
    settle(cx);
    assert!(
        !present(cx, "specs-projects"),
        "no line while every project follows"
    );

    turn(&user, "specs", false, Some("notes-app"));
    until(&shell, cx, "the line names the project", |cx| {
        settle(cx);
        label(cx, "specs-projects").is_some()
    });
    assert_eq!(
        label(cx, "specs-projects").as_deref(),
        Some("notes-app turns it off")
    );
    // The project is a link: its Settings open on the feature's page.
    click(cx, "specs-projects-notes-app");
    until(&shell, cx, "the project's settings open on it", |cx| {
        shell
            .read_with(cx, |s, cx| {
                s.main().read(cx).project_settings_page().cloned()
            })
            .is_some_and(|page| {
                page.read_with(cx, |page, _| {
                    page.name() == "notes-app"
                        && TABS[page.tab()] == "features"
                        && page.open_feature() == Some("specs")
                })
            })
    });
}

#[gpui_kit::test]
fn a_feature_that_turns_off_in_settings_fades_in_its_place_and_back(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    turn(&user, "review", true, None);
    turn(&user, "specs", true, None);
    let (shell, cx) = window(&user, cx);
    open_features(&user, &shell, cx);
    until(&shell, cx, "both rows are drawn", |cx| {
        visible(cx, "project-features-specs") && visible(cx, "project-features-review")
    });
    settle(cx);
    let (review, specs) = (
        top(cx, "project-features-review"),
        top(cx, "project-features-specs"),
    );
    assert!(!present(cx, "project-features-specs-off"));

    turn(&user, "specs", false, None);
    until(&shell, cx, "Specs begins to fade", |cx| {
        faded_by(cx, "project-features-specs-off").is_some()
    });
    run_for(Duration::from_millis(100), cx);
    let part = faded_by(cx, "project-features-specs-off");
    assert!(
        part.is_some_and(|part| 0.0 < part && part < 1.0),
        "the fade is on its way: {part:?}"
    );
    settle(cx);
    assert_eq!(faded_by(cx, "project-features-specs-off"), Some(1.0));
    assert_eq!(top(cx, "project-features-review"), review);
    assert_eq!(top(cx, "project-features-specs"), specs, "no card moved");

    turn(&user, "specs", true, None);
    until(&shell, cx, "Specs is on again", |cx| {
        settle(cx);
        !present(cx, "project-features-specs-off")
    });
    assert_eq!(top(cx, "project-features-specs"), specs);
}

#[gpui_kit::test]
fn each_section_of_a_tab_enters_in_its_turn(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let (shell, cx) = window(&user, cx);
    open_settings(&shell, "app", cx);
    let ids = ["workspaces", "branches", "setup"].map(|part| format!("enter-settings-app-{part}"));
    let y = |id: &String, cx: &mut VisualTestContext| top(cx, id);

    cx.update(|window, cx| window.click("project-settings-tabs-workspaces", cx));
    run_for(Duration::from_millis(300), cx);
    let waiting: Vec<f32> = ids.iter().map(|id| y(id, cx)).collect();
    settle(cx);
    let arrived: Vec<f32> = ids.iter().map(|id| y(id, cx)).collect();
    let rise = |at: usize| waiting[at] - arrived[at];
    assert!(
        rise(0) + 0.5 < rise(1) && rise(1) + 0.5 < rise(2),
        "each section has risen less than the one after it: {:?}",
        [rise(0), rise(1), rise(2)]
    );
}

#[gpui_kit::test]
fn the_tickets_open_settings_shows_the_tracker_in_view(cx: &mut TestAppContext) {
    let user = user();
    let folder = user.path("work/many");
    for at in 0..14 {
        let dir = folder.join(format!("r{at:02}"));
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), "start\n").unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    }
    user.call("project.add", json!({"folder": folder, "name": "many"}))
        .unwrap();
    let (shell, cx) = window(&user, cx);
    cx.simulate_resize(size(px(1280.), px(720.)));
    let in_view = |id: &str, cx: &mut VisualTestContext| {
        let id = id.to_owned();
        cx.update(|window, _| {
            let page = window.find("project-settings-page").bounds();
            let at = window.find(id).bounds();
            at.origin.y >= page.origin.y && at.bottom() <= page.bottom()
        })
    };

    open_settings(&shell, "many", cx);
    assert!(
        !in_view("tracker-title", cx),
        "the fourteen repositories push the tracker out of the window"
    );
    // The tickets' Open Settings, a repository's chip, the vault's link: all of them go here.
    shell.update_in(cx, |s, window, cx| {
        s.open_project_settings_at("many", Reveal::Tracker, window, cx)
    });
    settle(cx);
    assert!(
        in_view("tracker-title", cx),
        "the tracker is brought in view"
    );

    // Another tab starts at its top, not where the page was scrolled to.
    shell.update_in(cx, |s, window, cx| {
        s.open_project_settings_at("many", Reveal::Tasks, window, cx)
    });
    settle(cx);
    assert!(in_view("tasks-title", cx), "the tasks start at the top");
    shell.update_in(cx, |s, window, cx| {
        s.open_project_settings_at("many", Reveal::Tracker, window, cx)
    });
    run_for(Duration::from_millis(2000), cx);
    assert!(
        in_view("tracker-title", cx),
        "also from another tab, whose rows enter above it, and once they have"
    );
}

/// A project with fourteen repositories, so that its Tracker is below the fold of a short window.
fn many_repositories(user: &User) {
    let folder = user.path("work/many");
    for at in 0..14 {
        let dir = folder.join(format!("r{at:02}"));
        user.git_init(&dir);
        std::fs::write(dir.join("README.md"), "start\n").unwrap();
        user.git(&dir, &["add", "."]);
        user.git(&dir, &["commit", "--quiet", "-m", "start"]);
    }
    user.call("project.add", json!({"folder": folder, "name": "many"}))
        .unwrap();
}

/// Whether the element is inside the visible part of the project's page.
fn in_view(cx: &mut VisualTestContext, id: &str) -> bool {
    let id = id.to_owned();
    cx.update(|window, _| {
        let page = window.find("project-settings-page").bounds();
        let at = window.find(id).bounds();
        at.origin.y >= page.origin.y && at.bottom() <= page.bottom()
    })
}

#[gpui_kit::test]
fn a_tab_chosen_while_the_tracker_is_held_starts_at_its_top(cx: &mut TestAppContext) {
    let user = user();
    many_repositories(&user);
    let (shell, cx) = window(&user, cx);
    cx.simulate_resize(size(px(1280.), px(720.)));
    open_settings(&shell, "many", cx);

    shell.update_in(cx, |s, window, cx| {
        s.open_project_settings_at("many", Reveal::Tracker, window, cx)
    });
    run_for(Duration::from_millis(100), cx);
    // Another tab, well within the hold: its first section is the one in view, not its second.
    cx.update(|window, cx| window.click("project-settings-tabs-instructions", cx));
    run_for(Duration::from_millis(2000), cx);
    assert!(
        in_view(cx, "rules-title"),
        "Instructions start at Rules, the first of its sections"
    );
}

#[gpui_kit::test]
fn the_user_s_wheel_ends_the_hold_on_the_tracker(cx: &mut TestAppContext) {
    let user = user();
    many_repositories(&user);
    let (shell, cx) = window(&user, cx);
    cx.simulate_resize(size(px(1280.), px(720.)));
    open_settings(&shell, "many", cx);

    shell.update_in(cx, |s, window, cx| {
        s.open_project_settings_at("many", Reveal::Tracker, window, cx)
    });
    run_for(Duration::from_millis(100), cx);
    let page = cx.update(|window, _| window.find("project-settings-page").bounds());
    let held = top(cx, "tracker-title") - f32::from(page.origin.y);
    // The user reads what is above it: the wheel moves the page up by 200 px.
    cx.simulate_event(gpui_kit::ScrollWheelEvent {
        position: page.center(),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(200.))),
        ..Default::default()
    });
    run_for(Duration::from_millis(1500), cx);
    let after = top(cx, "tracker-title") - f32::from(page.origin.y);
    assert!(
        after > held + 100.0,
        "the page stays where the user put it: the tracker was {held} px under the top, now {after}"
    );
}

#[gpui_kit::test]
fn a_project_s_rules_show_no_save_at_rest_and_one_once_they_are_changed(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let (shell, cx) = window(&user, cx);
    let page = open_settings(&shell, "app", cx);
    tab(cx, "instructions");
    let editor = page.read_with(cx, |page, _| page.rules().clone());
    until(&shell, cx, "the rules are shown", |cx| {
        editor.read_with(cx, |e, _| e.shown().is_some())
    });
    settle(cx);
    assert!(!present(cx, "rules-save-app"), "no Save at rest");

    let field = editor
        .read_with(cx, |editor, _| editor.field().cloned())
        .unwrap();
    cx.update(|window, cx| {
        field.update(cx, |field, cx| {
            field.set_value("Name branches feat/<n>.", window, cx)
        })
    });
    cx.run_until_parked();
    settle(cx);
    assert!(visible(cx, "rules-save-app"), "Save unfolds once changed");

    cx.update(|window, cx| field.update(cx, |field, cx| field.set_value("", window, cx)));
    cx.run_until_parked();
    settle(cx);
    assert!(
        !present(cx, "rules-save-app"),
        "and folds away once set back"
    );
}
