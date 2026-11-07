//! The scenarios of #312: the top bar's search is a quick switcher (Ctrl K) that finds sessions,
//! projects with their workspaces, and actions. On GPUI's test platform in front of a real daemon
//! whose agents are the fake ACP agent, with a throwaway user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_ui::state::icon_id;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::shell::Selection;
use flottille_window::shell::switcher::{
    SWITCHER, Switcher, action_key, project_key, row_id, session_key, workspace_key,
};
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": []}));
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
}

fn project(user: &User, name: &str) {
    let folder = user.path(&format!("work/{name}"));
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), format!("{name}\n")).unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": name}))
        .unwrap();
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !check() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Starts a session whose first message is `message` and waits until its turn ended.
fn start(user: &User, project: &str, message: &str) -> String {
    let answer = user
        .call(
            "agent.start",
            json!({"project": project, "message": message}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || {
        user.call("agent.show", json!({"agent": id})).unwrap()["agent"]["state"] == "idle"
    });
    id
}

/// Two projects, a workspace in one of them and two sessions, the second the more recent one.
struct World {
    user: User,
    login: String,
    notes: String,
}

fn world() -> World {
    let user = user();
    project(&user, "app");
    project(&user, "harbor");
    let workspace = user
        .call(
            "workspace.create",
            json!({"project": "app", "name": "login-fix", "offline": true}),
        )
        .unwrap();
    assert_eq!(workspace["state"], "ready", "{workspace}");
    let login = start(&user, "app", "Fix the login bug");
    std::thread::sleep(Duration::from_millis(20));
    let notes = start(&user, "harbor", "Write the release notes");
    World { user, login, notes }
}

fn window<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let env = Environment {
        os_release: user.path("os-release"),
        state_home: user.path("state"),
    };
    cx.update(|cx| flottille_ui::init_with(env, cx));
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1280.), px(800.)));
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
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

/// The window with the tree loaded: both sessions are known.
fn ready<'a>(
    world: &World,
    cx: &'a mut TestAppContext,
) -> (Entity<Shell>, &'a mut VisualTestContext) {
    let (shell, cx) = window(&world.user, cx);
    let (login, notes) = (world.login.clone(), world.notes.clone());
    until(&shell, cx, "both sessions are in the tree", |cx| {
        cx.update(|window, _| {
            window.try_find(session_id(&login)).is_some()
                && window.try_find(session_id(&notes)).is_some()
        })
    });
    settle(cx);
    (shell, cx)
}

fn switcher(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<Switcher> {
    shell.read_with(cx, |shell, _| shell.switcher().clone())
}

fn is_open(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> bool {
    let switcher = switcher(shell, cx);
    switcher.read_with(cx, |switcher, _| switcher.is_open())
}

fn field_has_focus(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> bool {
    let field = switcher(shell, cx).read_with(cx, |switcher, _| switcher.field().clone());
    cx.update(|window, cx| field.read(cx).focus_handle(cx).is_focused(window))
}

fn said(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Vec<(String, Vec<String>)> {
    switcher(shell, cx).read_with(cx, |switcher, _| switcher.said())
}

fn marked(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<String> {
    switcher(shell, cx).read_with(cx, |switcher, _| switcher.marked().map(str::to_owned))
}

fn open_with_keys(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    cx.simulate_keystrokes("ctrl-k");
    settle(cx);
    assert!(is_open(shell, cx), "Ctrl K opens the switcher");
}

/// A frame drawn with no time passing.
fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn titles(said: &[(String, Vec<String>)], group: &str) -> Vec<String> {
    said.iter()
        .find(|(heading, _)| heading == group)
        .map(|(_, titles)| titles.clone())
        .unwrap_or_default()
}

/// How many frames the window still asks for: none at rest.
fn pending_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

#[gpui_kit::test]
fn ctrl_k_opens_the_switcher_with_its_field_focused(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    assert!(!is_open(&shell, cx));
    open_with_keys(&shell, cx);
    assert!(field_has_focus(&shell, cx), "typing goes to its field");
    assert!(cx.update(|window, _| window.try_find(SWITCHER).is_some()));
    // Pressed again, it closes: the same key opens and closes.
    cx.simulate_keystrokes("ctrl-k");
    settle(cx);
    assert!(!is_open(&shell, cx));
}

#[gpui_kit::test]
fn a_click_on_the_top_bar_search_opens_the_switcher(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    cx.update(|window, cx| window.click("top-search", cx));
    settle(cx);
    assert!(is_open(&shell, cx));
    assert!(field_has_focus(&shell, cx));
}

#[gpui_kit::test]
fn typing_a_session_s_title_lists_it_and_enter_opens_it(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    cx.simulate_input("LOGIN bug");
    settle(cx);
    let said = said(&shell, cx);
    assert_eq!(
        titles(&said, "Sessions"),
        ["Fix the login bug"],
        "case does not matter, nor the order in the words: {said:?}"
    );
    assert_eq!(marked(&shell, cx), Some(session_key(&world.login)));
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(!is_open(&shell, cx), "choosing closes it");
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.selected().cloned()),
        Some(Selection::Session(world.login.clone()))
    );
}

#[gpui_kit::test]
fn a_word_with_an_accent_is_found_without_it(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let id = start(&user, "app", "Écrire le résumé de la réunion");
    let world = World {
        user,
        login: id.clone(),
        notes: id,
    };
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    cx.simulate_input("resume reunion");
    settle(cx);
    assert_eq!(
        titles(&said(&shell, cx), "Sessions"),
        ["Écrire le résumé de la réunion"]
    );
}

#[gpui_kit::test]
fn the_results_are_grouped_sessions_projects_actions(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    cx.simulate_input("app");
    settle(cx);
    let said = said(&shell, cx);
    let headings: Vec<&str> = said.iter().map(|(heading, _)| heading.as_str()).collect();
    assert_eq!(headings, ["Sessions", "Projects", "Actions"], "{said:?}");
    // The session in the project `app`, found by its project; its title has no `app` in it.
    assert_eq!(titles(&said, "Sessions"), ["Fix the login bug"]);
    // The project and its workspace (found by its project's name) are in one group.
    assert_eq!(titles(&said, "Projects"), ["app", "login-fix"]);
    assert_eq!(
        titles(&said, "Actions"),
        ["Settings: Appearance", "Settings of app"]
    );
    for heading in ["Sessions", "Projects", "Actions"] {
        assert!(
            cx.update(|window, _| window
                .try_find(format!("switcher-heading-{}", heading.to_lowercase()))
                .is_some()),
            "{heading} has its heading"
        );
    }
}

#[gpui_kit::test]
fn choosing_a_project_or_a_workspace_opens_its_page(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    cx.simulate_input("harbor");
    settle(cx);
    cx.update(|window, cx| window.click(row_id(&project_key("harbor")), cx));
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.selected().cloned()),
        Some(Selection::Project("harbor".into()))
    );
    assert!(!is_open(&shell, cx), "a click on a result closes it");

    open_with_keys(&shell, cx);
    cx.simulate_input("login-fix");
    settle(cx);
    cx.update(|window, cx| window.click(row_id(&workspace_key("app", "login-fix")), cx));
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.selected().cloned()),
        Some(Selection::Project("app".into())),
        "a workspace shows on its project's page"
    );
}

#[gpui_kit::test]
fn an_action_is_done_when_chosen(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    cx.simulate_input("add a project");
    settle(cx);
    assert_eq!(marked(&shell, cx), Some(action_key("Add a project")));
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(!is_open(&shell, cx));
    let form = shell.read_with(cx, |shell, _| shell.add_project_form().clone());
    assert!(
        form.read_with(cx, |form, _| form.is_open()),
        "its form is open"
    );

    // A tab of Settings is an action too: it opens on that tab.
    open_with_keys(&shell, cx);
    cx.simulate_input("settings appearance");
    settle(cx);
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.selected().cloned()),
        Some(Selection::Settings)
    );
    let page = shell
        .read_with(cx, |shell, cx| {
            shell.main().read(cx).settings_page().cloned()
        })
        .expect("Settings is open");
    let appearance = flottille_window::settings::SECTIONS
        .iter()
        .position(|(key, _)| *key == "appearance")
        .unwrap();
    assert_eq!(
        page.read_with(cx, |page, _| page.tab()),
        appearance,
        "the Appearance tab"
    );
}

#[gpui_kit::test]
fn the_switcher_opens_the_judge_tab_with_the_guidance_in_view(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    // A window too short for the judge and its guidance together.
    cx.simulate_resize(size(px(1280.), px(520.)));
    open_with_keys(&shell, cx);
    cx.simulate_input("settings guidance");
    settle(cx);
    assert_eq!(
        marked(&shell, cx),
        Some(action_key("Settings: Judge guidance"))
    );
    cx.simulate_keystrokes("enter");
    settle(cx);
    let page = shell
        .read_with(cx, |shell, cx| {
            shell.main().read(cx).settings_page().cloned()
        })
        .expect("Settings is open");
    assert_eq!(page.read_with(cx, |page, _| page.tab()), 0, "the Judge tab");
    cx.update(|window, _| {
        let text = window.find("judge-guidance-text").bounds();
        assert!(
            text.top() >= px(0.) && text.bottom() <= window.viewport_size().height,
            "the guidance is in view"
        );
    });
}

#[gpui_kit::test]
fn escape_closes_the_switcher_and_gives_the_focus_back(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    let window_focus = shell.read_with(cx, |shell, _| shell.focus_handle());
    cx.update(|window, cx| window.focus(&window_focus, cx));
    open_with_keys(&shell, cx);
    assert!(!cx.update(|window, _| window_focus.is_focused(window)));
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(!is_open(&shell, cx));
    assert!(
        cx.update(|window, _| window_focus.is_focused(window)),
        "what had the focus has it again"
    );
    assert!(cx.update(|window, _| window.try_find(SWITCHER).is_none()));
}

#[gpui_kit::test]
fn a_click_outside_closes_it(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    cx.update(|window, cx| window.click(format!("{SWITCHER}-scrim"), cx));
    settle(cx);
    assert!(!is_open(&shell, cx));
}

#[gpui_kit::test]
fn with_nothing_typed_the_recent_sessions_and_the_actions_are_listed(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    let said = said(&shell, cx);
    let headings: Vec<&str> = said.iter().map(|(heading, _)| heading.as_str()).collect();
    assert_eq!(
        headings,
        ["Sessions", "Actions"],
        "no project until one is asked for"
    );
    assert_eq!(
        titles(&said, "Sessions"),
        ["Write the release notes", "Fix the login bug"],
        "the most recent first"
    );
    let actions = titles(&said, "Actions");
    for wanted in [
        "New session",
        "Add a project",
        "Settings",
        "Settings: Judge",
    ] {
        assert!(
            actions.iter().any(|a| a == wanted),
            "{wanted} in {actions:?}"
        );
    }
    assert_eq!(marked(&shell, cx), Some(session_key(&world.notes)));
}

#[gpui_kit::test]
fn the_arrows_move_the_mark_and_the_pointer_marks_the_row_it_is_on(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    assert_eq!(marked(&shell, cx), Some(session_key(&world.notes)));
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(marked(&shell, cx), Some(session_key(&world.login)));
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(
        marked(&shell, cx),
        Some(action_key("New session")),
        "into the next group"
    );
    cx.simulate_keystrokes("up up");
    settle(cx);
    assert_eq!(marked(&shell, cx), Some(session_key(&world.notes)));
    cx.simulate_keystrokes("up");
    settle(cx);
    assert_eq!(
        marked(&shell, cx),
        Some(session_key(&world.notes)),
        "it stops at the first"
    );
    // The field keeps the focus: what is typed still reaches it.
    assert!(field_has_focus(&shell, cx));

    cx.update(|window, cx| window.hover(row_id(&action_key("Add a project")), cx));
    settle(cx);
    assert_eq!(marked(&shell, cx), Some(action_key("Add a project")));
}

#[gpui_kit::test]
fn nothing_matching_says_so_and_nothing_is_redrawn_at_rest(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    cx.simulate_input("zzzzqq");
    settle(cx);
    assert!(said(&shell, cx).is_empty());
    assert_eq!(marked(&shell, cx), None);
    assert!(cx.update(|window, _| window.try_find("switcher-none").is_some()));
    // Enter with nothing marked does nothing.
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(is_open(&shell, cx));
    assert_eq!(pending_frames(cx), 0, "nothing moves at rest");
}

#[gpui_kit::test]
fn the_top_bar_search_is_a_labelled_hoverable_field(cx: &mut TestAppContext) {
    let world = world();
    let (_shell, cx) = ready(&world, cx);
    let search = cx.update(|window, _| window.find("top-search"));
    assert!(
        search.label().is_some_and(|label| label.contains("Search")),
        "{search:?}"
    );
    // Hover fades: the first frames after the pointer arrives still ask for frames.
    cx.update(|window, cx| window.hover("top-search", cx));
    assert!(pending_frames(cx) > 0, "the hover fades in");
    settle(cx);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn closed_from_the_window_with_nothing_focused_the_window_gets_the_focus(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    // What had the focus is gone (a composer whose session left the tree): nothing has it.
    cx.update(|window, cx| window.blur(cx));
    let found = switcher(&shell, cx);
    cx.update(|window, cx| found.update(cx, |switcher, cx| switcher.open(window, cx)));
    settle(cx);
    assert!(is_open(&shell, cx));
    // Ctrl K or the top bar closes it, from inside the shell's own handling of it.
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.toggle_switcher(window, cx)));
    settle(cx);
    assert!(!is_open(&shell, cx));
    let window_focus = shell.read_with(cx, |shell, _| shell.focus_handle());
    assert!(
        cx.update(|window, _| window_focus.is_focused(window)),
        "nothing had it: the window has it"
    );
}

#[gpui_kit::test]
fn escape_gives_the_focus_back_to_the_composer_that_had_it(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.open(Selection::Project("app".into()), window, cx)
        })
    });
    settle(cx);
    let composer = shell
        .read_with(cx, |shell, _| shell.composer_of("app").cloned())
        .expect("the project's page has its composer");
    let field = composer.read_with(cx, |composer, _| composer.field().clone());
    let focus = cx.update(|_, cx| field.read(cx).focus_handle(cx));
    cx.update(|window, cx| window.focus(&focus, cx));
    settle(cx);
    // Ctrl K reaches the window from a text field elsewhere.
    open_with_keys(&shell, cx);
    assert!(field_has_focus(&shell, cx));
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(
        cx.update(|window, _| focus.is_focused(window)),
        "the composer has the focus again"
    );
}

#[gpui_kit::test]
fn escape_gives_the_focus_to_the_window_when_what_had_it_is_no_longer_drawn(
    cx: &mut TestAppContext,
) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    // A handle that is alive but that no frame draws any more: a composer whose session left the
    // snapshot while the switcher was open still has its handle held by the switcher.
    let gone = cx.update(|_, cx| cx.focus_handle());
    cx.update(|window, cx| window.focus(&gone, cx));
    settle(cx);
    let found = switcher(&shell, cx);
    cx.update(|window, cx| found.update(cx, |switcher, cx| switcher.open(window, cx)));
    settle(cx);
    assert!(is_open(&shell, cx));
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(!is_open(&shell, cx));
    let window_focus = shell.read_with(cx, |shell, _| shell.focus_handle());
    assert!(
        cx.update(|window, _| window_focus.is_focused(window)),
        "the window has the focus, not the handle nothing draws"
    );
    // Keys still reach the window: Ctrl K opens it again.
    cx.simulate_keystrokes("ctrl-k");
    settle(cx);
    assert!(is_open(&shell, cx), "keys reach the window");
}

#[gpui_kit::test]
fn an_arrow_right_after_typing_scrolls_to_the_marked_row(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    cx.simulate_resize(size(px(1280.), px(420.)));
    settle(cx);
    open_with_keys(&shell, cx);
    cx.simulate_input("settings");
    // No time passes: the two sessions and their heading that left are still folding away, above
    // the results, and still drawn.
    assert!(titles(&said(&shell, cx), "Sessions").is_empty());
    assert!(
        cx.update(|window, _| window
            .try_find(row_id(&session_key(&world.login)))
            .is_some()),
        "a result that left is still drawn"
    );
    // The frame that draws them is the one the arrow's scroll reads the rows' places from.
    draw(cx);
    cx.simulate_keystrokes(&"down ".repeat(40));
    draw(cx);
    draw(cx);
    // The rows that left fold away: the scroll made in that frame leaves the marked row in sight
    // once they are gone, where it ends up as they fold.
    settle(cx);
    let last = marked(&shell, cx).expect("a result is marked");
    // The row's place is the one drawn, the scroll already in it.
    let row = cx.update(|window, _| window.find(row_id(&last)).bounds());
    let viewport = switcher(&shell, cx).read_with(cx, |switcher, _| switcher.scroll().bounds());
    assert!(
        row.top() >= viewport.top() && row.bottom() <= viewport.bottom(),
        "the marked row {row:?} is in sight in {viewport:?}"
    );
}

#[gpui_kit::test]
fn the_best_match_is_marked_whatever_its_group(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "app");
    let one = start(
        &user,
        "app",
        "Set up the staging tests, then investigate why the signup flow times out on slow links",
    );
    let two = start(
        &user,
        "app",
        "Rewrite the session timeout handling so that idle tabs never sign the user out twice",
    );
    let world = World {
        user,
        login: one,
        notes: two,
    };
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    cx.simulate_input("settings");
    settle(cx);
    assert_eq!(
        marked(&shell, cx),
        Some(action_key("Settings")),
        "{:?}",
        said(&shell, cx)
    );
    assert!(
        titles(&said(&shell, cx), "Sessions").is_empty(),
        "letters scattered over a long title are no match"
    );
}

#[gpui_kit::test]
fn a_read_of_the_daemon_while_open_keeps_the_order_and_the_mark(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    let order = titles(&said(&shell, cx), "Sessions");
    assert_eq!(order, ["Write the release notes", "Fix the login bug"]);
    cx.simulate_keystrokes("down");
    settle(cx);
    assert_eq!(marked(&shell, cx), Some(session_key(&world.login)));
    // The older session becomes the most recent one.
    let before = shell.read_with(cx, |shell, _| {
        shell
            .snapshot()
            .sessions
            .iter()
            .find(|s| s.id == world.login)
            .map(|s| s.active_at)
            .unwrap()
    });
    world
        .user
        .call(
            "agent.send",
            json!({"agent": world.login, "message": "and the logout too"}),
        )
        .unwrap();
    let login = world.login.clone();
    until(&shell, cx, "the window reads the new activity", |cx| {
        let shell = shell.clone();
        cx.update(|_, cx| {
            shell
                .read(cx)
                .snapshot()
                .sessions
                .iter()
                .any(|s| s.id == login && s.active_at > before)
        })
    });
    settle(cx);
    assert!(is_open(&shell, cx));
    assert_eq!(
        titles(&said(&shell, cx), "Sessions"),
        order,
        "nothing moves under the pointer"
    );
    assert_eq!(marked(&shell, cx), Some(session_key(&world.login)));
}

#[gpui_kit::test]
fn ctrl_k_does_nothing_while_a_dialog_is_open(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.add_project(window, cx)));
    settle(cx);
    let form = shell.read_with(cx, |shell, _| shell.add_project_form().clone());
    assert!(form.read_with(cx, |form, _| form.is_open()));
    cx.simulate_keystrokes("ctrl-k");
    settle(cx);
    assert!(!is_open(&shell, cx), "the dialog keeps the keys");
    assert!(form.read_with(cx, |form, _| form.is_open()));
}

#[gpui_kit::test]
fn a_project_s_settings_and_the_bell_are_actions(cx: &mut TestAppContext) {
    let world = world();
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    cx.simulate_input("harbor settings");
    settle(cx);
    assert_eq!(
        marked(&shell, cx),
        Some(action_key("Settings of harbor")),
        "{:?}",
        said(&shell, cx)
    );
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |shell, _| shell.selected().cloned()),
        Some(Selection::ProjectSettings("harbor".into()))
    );

    open_with_keys(&shell, cx);
    cx.simulate_input("notifications");
    settle(cx);
    assert_eq!(marked(&shell, cx), Some(action_key("Notifications")));
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(
        shell.read_with(cx, |shell, _| shell.bell_open()),
        "the bell's list is open"
    );
}

#[test]
fn a_workspace_s_key_is_its_own() {
    assert_ne!(
        workspace_key("app-b", "c"),
        workspace_key("app", "b-c"),
        "a dash in a name does not make two keys one"
    );
}

#[gpui_kit::test]
fn the_top_bar_search_moves_when_pressed(cx: &mut TestAppContext) {
    let world = world();
    let (_shell, cx) = ready(&world, cx);
    let bounds = cx.update(|window, _| window.find("top-search").bounds());
    let at = bounds.center();
    cx.simulate_mouse_move(at, None, gpui_kit::Modifiers::default());
    settle(cx);
    cx.simulate_mouse_down(
        at,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::default(),
    );
    assert!(pending_frames(cx) > 0, "the press moves");
    settle(cx);
    cx.simulate_mouse_up(
        at,
        gpui_kit::MouseButton::Left,
        gpui_kit::Modifiers::default(),
    );
    // The click opens the switcher, whose rows come in one after another.
    settle(cx);
    settle(cx);
    assert_eq!(pending_frames(cx), 0);
}

#[gpui_kit::test]
fn a_session_s_state_is_the_tree_s_icon_in_its_result(cx: &mut TestAppContext) {
    let user = user();
    user.fake_script(json!({"turns": [
        {"when": "read the env", "do": [{"mcp": {"tool": "fs_read", "arguments": {"path": ".env"}}}]},
    ]}));
    project(&user, "app");
    let id = start(&user, "app", "Fix the login bug");
    let world = World {
        user,
        login: id.clone(),
        notes: id.clone(),
    };
    let (shell, cx) = ready(&world, cx);
    open_with_keys(&shell, cx);
    let row = row_id(&session_key(&id));
    let icon_in_row = |cx: &mut VisualTestContext| {
        let row = row.clone();
        cx.update(|window, _| {
            window
                .within(row)
                .try_find(icon_id(AgentState::NeedsYou))
                .is_some()
        })
    };
    assert!(!icon_in_row(cx), "at rest: how long ago");
    world
        .user
        .call(
            "agent.send",
            json!({"agent": id, "message": "read the env"}),
        )
        .unwrap();
    until(&shell, cx, "the result shows the state icon", |cx| {
        icon_in_row(cx)
    });
    assert!(is_open(&shell, cx));
}
