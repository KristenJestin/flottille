//! The scenarios of #186, the project page's workspace cards: "New session in …" at the card's
//! bottom, a "⋯" menu on each session row that ends it, and no ended session in the cards. On
//! GPUI's test platform in front of a real daemon whose agents are the fake ACP agent, with a
//! throwaway user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::link::Connect;
use flottille_window::project::{
    PlaceCard, ProjectPage, card_session_end_id, card_session_menu_id,
};
use flottille_window::shell::Selection;
use flottille_window::tree::ended_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::json;

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(json!({"turns": [
        {"when": "work briefly", "do": [{"wait": 1500}, {"say": "done"}]}
    ]}));
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

fn state_of(user: &User, agent: &str) -> String {
    user.call("agent.show", json!({"agent": agent})).unwrap()["agent"]["state"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn workspace(user: &User, project: &str, name: &str) {
    user.call(
        "workspace.create",
        json!({"project": project, "name": name}),
    )
    .unwrap();
    eventually("the workspace is ready", || {
        user.call("workspace.show", json!({"project": project, "name": name}))
            .unwrap()["state"]
            == "ready"
    });
}

/// Starts a session in `workspace` (main when none) and waits until its first turn ended.
fn start(user: &User, project: &str, workspace: Option<&str>, message: &str) -> String {
    let mut params = json!({"project": project, "message": message});
    if let Some(workspace) = workspace {
        params["workspace"] = json!(workspace);
    }
    let answer = user.call("agent.start", params).unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("the first turn ends", || state_of(user, &id) == "idle");
    id
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
    // Two columns, and tall enough for the cards of the first row.
    cx.simulate_resize(size(px(1280.), px(1000.)));
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

fn page_of(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<ProjectPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).project_page().cloned())
        .expect("a project's page is open")
}

fn card_of(
    shell: &Entity<Shell>,
    place: Option<&str>,
    cx: &mut VisualTestContext,
) -> Entity<PlaceCard> {
    let page = page_of(shell, cx);
    page.read_with(cx, |page, cx| page.card(&place.map(str::to_owned), cx))
        .expect("the card is on the page")
}

/// The ids of the sessions a card lists.
fn listed(shell: &Entity<Shell>, place: Option<&str>, cx: &mut VisualTestContext) -> Vec<String> {
    card_of(shell, place, cx).read_with(cx, |card, _| {
        card.data().sessions.iter().map(|s| s.id.clone()).collect()
    })
}

fn open_project(shell: &Entity<Shell>, project: &str, cx: &mut VisualTestContext) {
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
}

fn has(id: impl Into<ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn row_id(session: &str) -> String {
    format!("card-session-{session}")
}

#[gpui_kit::test]
fn new_session_in_sits_at_the_bottom_of_a_card_taller_than_its_content(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "alpha");
    let only = start(&user, "atlas", None, "Release notes");
    for message in ["One", "Two", "Three", "Four"] {
        start(&user, "atlas", Some("alpha"), message);
    }
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the sessions are on the cards", |cx| {
        listed(&shell, None, cx).len() == 1 && listed(&shell, Some("alpha"), cx).len() == 4
    });
    settle(cx);

    cx.update(|window, _| {
        let (main, alpha) = (
            window.find("place-card-main").bounds(),
            window.find("place-card-alpha").bounds(),
        );
        assert_eq!(main.origin.y, alpha.origin.y, "one row");
        assert_eq!(
            main.size.height, alpha.size.height,
            "the cards of a row share a height"
        );
        let last = window.find(row_id(&only)).bounds();
        let new_session = window.find("new-session-in-main").bounds();
        assert!(
            new_session.origin.y - last.bottom() > px(80.),
            "the card is taller than its content: {:?} under {:?}",
            new_session,
            last
        );
        // The card's border is one pixel.
        assert!(
            (main.bottom() - new_session.bottom()).abs() <= px(2.),
            "the line sits on the card's bottom: {:?} in {:?}",
            new_session,
            main
        );
        let alpha_new = window.find("new-session-in-alpha").bounds();
        assert_eq!(
            alpha_new.bottom(),
            new_session.bottom(),
            "the two cards' lines are on one level"
        );
    });
}

#[gpui_kit::test]
fn a_session_row_ends_its_session_from_its_menu_and_the_row_folds_away(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let keep = start(&user, "atlas", None, "Release notes");
    let end = start(&user, "atlas", None, "Review the refresh flow");
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "both sessions are on the card", |cx| {
        listed(&shell, None, cx).len() == 2
    });
    settle(cx);

    // The menu is the row's, under the pointer: it does not open the session.
    cx.update(|window, cx| window.hover(row_id(&end), cx));
    settle(cx);
    cx.update(|window, cx| window.click(card_session_menu_id(&end), cx));
    settle(cx);
    cx.update(|window, cx| window.click(card_session_end_id(&end), cx));
    settle(cx);
    assert!(
        !matches!(
            shell.read_with(cx, |s, _| s.selected().cloned()),
            Some(Selection::Session(_))
        ),
        "no session opened"
    );
    cx.update(|window, _| assert!(window.find("sidebar-end-modal").visible()));

    // Cancel ends nothing.
    cx.update(|window, cx| window.click("sidebar-end-modal-cancel", cx));
    settle(cx);
    assert_eq!(state_of(&user, &end), "idle");
    assert_eq!(listed(&shell, None, cx).len(), 2);

    // Confirmed, it ends and its row folds away, the other staying.
    cx.update(|window, cx| window.hover(row_id(&end), cx));
    settle(cx);
    cx.update(|window, cx| window.click(card_session_menu_id(&end), cx));
    settle(cx);
    cx.update(|window, cx| window.click(card_session_end_id(&end), cx));
    settle(cx);
    cx.update(|window, cx| window.click("sidebar-end-confirm", cx));
    until(&shell, cx, "the session left the card", |cx| {
        listed(&shell, None, cx) == [keep.clone()]
    });
    assert_eq!(state_of(&user, &end), "ended");
    assert!(
        has(row_id(&end), cx),
        "its row is still drawn while it folds away"
    );
    settle(cx);
    assert!(!has(row_id(&end), cx), "its row is gone once it folded");
    assert!(has(row_id(&keep), cx));
    assert!(
        has(ended_id(), cx),
        "it is in the sidebar's Ended, which opens it"
    );
}

#[gpui_kit::test]
fn a_click_on_a_session_row_still_opens_the_session(cx: &mut TestAppContext) {
    let user = user();
    project(&user, "atlas");
    let id = start(&user, "atlas", None, "Release notes");
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the session is on the card", |cx| {
        listed(&shell, None, cx).len() == 1
    });
    settle(cx);
    cx.update(|window, cx| window.click(row_id(&id), cx));
    settle(cx);
    assert_eq!(
        shell.read_with(cx, |s, _| s.selected().cloned()),
        Some(Selection::Session(id))
    );
}

#[gpui_kit::test]
fn an_ended_session_is_not_listed_in_a_card_and_stays_in_the_sidebar_s_ended(
    cx: &mut TestAppContext,
) {
    let user = user();
    project(&user, "atlas");
    workspace(&user, "atlas", "alpha");
    let live = start(&user, "atlas", None, "Release notes");
    let gone = start(&user, "atlas", None, "Old idea");
    let gone_in_alpha = start(&user, "atlas", Some("alpha"), "Old idea in alpha");
    user.call("agent.stop", json!({"agent": gone})).unwrap();
    user.call("agent.stop", json!({"agent": gone_in_alpha}))
        .unwrap();
    let (shell, cx) = window(&user, Arc::new(user.connector()), cx);
    open_project(&shell, "atlas", cx);
    until(&shell, cx, "the sidebar's Ended group", |cx| {
        has(ended_id(), cx)
    });
    settle(cx);

    assert_eq!(listed(&shell, None, cx), std::slice::from_ref(&live));
    assert_eq!(
        listed(&shell, Some("alpha"), cx),
        Vec::<String>::new(),
        "a workspace whose sessions all ended lists none"
    );
    assert!(has(row_id(&live), cx));
    assert!(!has(row_id(&gone), cx));
    assert!(!has(row_id(&gone_in_alpha), cx));
}
