//! The scenarios of #519 in the window: a question a root asks the user with `ask_user` in the
//! middle of its turn shows at once, as a card at its place in the conversation with the field
//! that answers it, in the bell and as "needs you" on the session's row, until its answer is
//! taken. On GPUI's test platform in front of a real daemon whose agents are the fake ACP agent,
//! with a throwaway user. No window opens on a screen.

mod common;
mod guard;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::question_card::{ANSWER_WAITS, QuestionCard};
use flottille_window::session::SessionPage;
use flottille_window::state::AgentState;
use flottille_window::tree::session_id;
use gpui_design_system::omarchy::Environment;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt};
use gpui_kit::{Entity, Focusable as _, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

const QUESTION: &str =
    "**POC charts**: blocked by the minimum of 3 points?\n\n- keep the minimum\n- drop it";
const SECOND: &str = "Which colour for the axis?";
const PUSH: &str = "Push `feat/42-export` and open the pull request?";
const TITLE: &str = "Title of the pull request?";
const REVIEWS: &str = "Who reviews it?";
const LONG_OPTION: &str = "You drop me a bundle or a clone of the real repository, and I replay \
    its commits one by one so that the history stays whole in the new place, with every author \
    and every date as they were.";

/// The agent asks, goes on with a call and a sentence, then works until the test lets it end.
fn script() -> Value {
    let ask = |q: &str| json!({"mcp": {"tool": "ask_user", "arguments": {"question": q}}});
    json!({"turns": [
        {"when": "draw the charts", "do": [
            ask(QUESTION),
            {"mcp": {"tool": "fs_list", "arguments": {"path": "."}}},
            {"say": "Looking further meanwhile."},
            {"mcp": {"tool": "fs_list", "arguments": {"path": "."}}},
            {"hold": "release"},
            {"say": "Done for now."}]},
        {"when": "ask twice", "do": [
            ask(QUESTION),
            {"say": "And one more."},
            ask(SECOND),
            {"hold": "release"}]},
        {"when": "ship it", "do": [
            {"mcp": {"tool": "ask", "arguments": {"questions": [
                {"question": PUSH,
                 "options": [{"label": "Yes"}, {"label": "Not yet", "description": "Keep it local"}]},
                {"question": TITLE},
                {"question": REVIEWS, "multiple": true,
                 "options": [{"label": "Alice"}, {"label": "Bob"}, {"label": "Carol"}]}
            ]}}},
            {"say": "Asked."}]},
        {"when": "pick one", "do": [
            {"mcp": {"tool": "ask", "arguments": {"questions": [
                {"question": PUSH, "options": [{"label": "Yes"}, {"label": "Not yet"}]}
            ]}}},
            {"say": "Asked."}]},
        {"when": "long options", "do": [
            {"mcp": {"tool": "ask", "arguments": {"questions": [
                {"question": PUSH, "options": [
                    {"label": "I send you the history", "description": LONG_OPTION},
                    {"label": "One initial commit"}]}
            ]}}},
            {"say": "Asked."}]},
        {"when": "long ask", "do": [
            {"mcp": {"tool": "ask", "arguments": {"questions": [
                {"question": format!("{} {}", guard::UNBROKEN, guard::SPACED), "options": [
                    {"label": &guard::UNBROKEN[..58], "description": guard::SPACED},
                    {"label": &guard::SPACED[..58]}]},
                {"question": guard::SPACED, "multiple": true, "options": [
                    {"label": "A"}, {"label": &guard::UNBROKEN[..58]}]}
            ]}}},
            {"say": "Asked."}]},
        {"when": "Drop it", "do": [{"say": "Dropped."}]},
        {"when": "Export orders", "do": [{"say": "Shipping."}]}
    ]})
}

fn user() -> User {
    let user = User::with_fake_agent();
    user.fake_script(script());
    std::fs::write(user.path("os-release"), "NAME=Test\nID=test\n").unwrap();
    user
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

/// A session of the project `app` over a throwaway repository, in `mode`, started with
/// `message`: its id, once its first turn ran.
fn session(user: &User, mode: &str, message: &str) -> String {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "."]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let answer = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap().to_owned();
    eventually("its first turn", || state_of(user, &id) == "idle");
    user.call("agent.mode", json!({"agent": id, "mode": mode}))
        .unwrap();
    user.call("agent.send", json!({"agent": id, "message": message}))
        .unwrap();
    id
}

fn release(user: &User) {
    std::fs::write(user.path("fake/release"), "").unwrap();
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
    let connect: Connect = Arc::new(user.connector());
    let (shell, cx) = cx.add_window_view(|window, cx| Shell::new_polled(connect, window, cx));
    cx.simulate_resize(size(px(1440.), px(800.)));
    (shell, cx)
}

/// Runs the window until `check` holds: takes what the links sent, then draws what changed.
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

/// Lets every animation end.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
    }
}

fn at_rest(cx: &mut VisualTestContext) {
    settle(cx);
    assert_eq!(
        cx.update(|window, cx| window.simulate_next_frame(cx)),
        0,
        "nothing moves at rest"
    );
}

fn has(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> bool {
    let id = id.into();
    cx.update(|window, _| window.try_find(id).is_some())
}

fn label(id: impl Into<gpui_kit::ElementId>, cx: &mut VisualTestContext) -> Option<String> {
    let id = id.into();
    cx.update(|window, _| {
        window
            .try_find(id)
            .and_then(|found| found.label().map(str::to_owned))
    })
}

fn session_page(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Entity<SessionPage> {
    shell
        .read_with(cx, |s, cx| s.main().read(cx).session_page().cloned())
        .expect("a session's page is open")
}

fn open_session<'a>(
    user: &User,
    agent: &str,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Shell>,
    Entity<SessionPage>,
    &'a mut VisualTestContext,
) {
    let (shell, cx) = window(user, cx);
    until(&shell, cx, "the session is in the tree", |cx| {
        has(session_id(agent), cx)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(agent), cx));
    until(&shell, cx, "the session's page shows", |cx| {
        session_page(&shell, cx).read_with(cx, |page, cx| {
            !page.conversation().read(cx).rows().is_empty() && !page.tree().is_empty()
        })
    });
    let page = session_page(&shell, cx);
    (shell, page, cx)
}

fn blocks(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<Block> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).blocks().to_vec()
    })
}

/// The rows of the questions of the conversation, in order, with their texts.
fn questions(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Vec<(usize, String)> {
    blocks(page, cx)
        .into_iter()
        .enumerate()
        .filter_map(|(at, block)| match block {
            Block::Question { text, .. } => Some((at, text)),
            _ => None,
        })
        .collect()
}

/// Whether the card of the question of row `at` shows, with its first question.
fn card_shows(at: usize, cx: &mut VisualTestContext) -> bool {
    has(format!("row-{at}-question-text"), cx) && has(format!("row-{at}-question-0-text"), cx)
}

fn mark_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Option<AgentState> {
    shell.read_with(cx, |shell, cx| {
        shell.session(id).and_then(|row| row.read(cx).mark())
    })
}

/// The bell's items of the session: their labels and excerpts.
fn bell_of(shell: &Entity<Shell>, id: &str, cx: &mut VisualTestContext) -> Vec<(String, String)> {
    shell.read_with(cx, |s, _| {
        s.snapshot()
            .bell
            .iter()
            .filter(|item| item.session == id)
            .map(|item| (item.label.clone(), item.detail.clone()))
            .collect()
    })
}

#[gpui_kit::test]
fn a_question_asked_mid_turn_shows_at_once_as_a_card(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "ask", "draw the charts");
    let (shell, page, cx) = open_session(&user, &id, cx);

    until(&shell, cx, "the card of the question", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
    });
    // The agent went on: a sentence after the question, the turn still running.
    until(&shell, cx, "what it said after", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Text { text } if text == "Looking further meanwhile."))
    });
    settle(cx);
    assert_eq!(state_of(&user, &id), "working");
    let (at, text) = questions(&page, cx).remove(0);
    assert_eq!(text, QUESTION, "the question in full");
    assert!(card_shows(at, cx), "the card stays while the agent works");
    assert_eq!(
        label(format!("row-{at}-answer-send"), cx).as_deref(),
        Some("Answer")
    );
    let rendered = page.read_with(cx, |page, cx| {
        page.conversation().read(cx).rows()[at]
            .read(cx)
            .rendered(cx)
    });
    assert!(rendered.is_none(), "{rendered:?}");
    release(&user);
    until(&shell, cx, "the turn's end", |_| {
        state_of(&user, &id) == "idle"
    });
    settle(cx);
    assert!(
        card_shows(at, cx),
        "it waits for the answer after the turn too"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn a_question_asked_mid_turn_is_in_the_inbox_and_marks_the_row(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "ask", "draw the charts");
    let (shell, cx) = window(&user, cx);

    until(
        &shell,
        cx,
        "the row needs the user and the bell rings",
        |cx| {
            mark_of(&shell, &id, cx) == Some(AgentState::NeedsYou)
                && !bell_of(&shell, &id, cx).is_empty()
        },
    );
    assert_eq!(state_of(&user, &id), "working", "mid-turn");
    assert_eq!(
        bell_of(&shell, &id, cx),
        [("Asks you a question".to_owned(), QUESTION.to_owned())]
    );
    let said = shell.read_with(cx, |s, _| {
        s.snapshot().session(&id).unwrap().mark_said.clone()
    });
    assert_eq!(said, "Needs you: a question");
    release(&user);
}

#[gpui_kit::test]
fn a_question_asked_in_full_mode_shows_like_in_ask_mode(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "draw the charts");
    let (shell, page, cx) = open_session(&user, &id, cx);

    until(&shell, cx, "the card, the mark and the bell", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
            && mark_of(&shell, &id, cx) == Some(AgentState::NeedsYou)
            && bell_of(&shell, &id, cx).len() == 1
    });
    assert_eq!(state_of(&user, &id), "working");
    release(&user);
}

#[gpui_kit::test]
fn answering_the_card_delivers_the_answer_and_folds_it(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "draw the charts");
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
    });
    let at = questions(&page, cx)[0].0;
    // Answered while the agent works: it waits for the end of the turn, and says so.
    let answer = answer(&page, at, "Drop it.", cx);
    until(&shell, cx, "the answer went", |cx| {
        answer.read_with(cx, |a, cx| a.is_sent(cx))
    });
    settle(cx);
    assert_eq!(
        label(format!("row-{at}-answer-waits"), cx).as_deref(),
        Some(ANSWER_WAITS)
    );
    assert!(has(format!("row-{at}-question-text"), cx), "still open");

    // The turn ends, the agent takes the answer: the card folds to it, the row and the bell
    // no longer say it.
    release(&user);
    until(&shell, cx, "the answer is taken", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Text { text } if text == "Dropped."))
    });
    until(&shell, cx, "the card folds to its answer", |cx| {
        settle(cx);
        !has(format!("row-{at}-question-text"), cx)
            && label(format!("row-{at}-answered-0-answer"), cx).as_deref() == Some("Drop it.")
            && mark_of(&shell, &id, cx) != Some(AgentState::NeedsYou)
            && bell_of(&shell, &id, cx).is_empty()
    });
    let prompts: Vec<String> = user
        .received_requests("session/prompt")
        .iter()
        .map(|r| r["params"]["prompt"].to_string())
        .collect();
    assert!(
        prompts.iter().any(|p| p.contains("Drop it.")),
        "{prompts:?}"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn two_open_questions_both_show_in_order(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "ask twice");
    let (shell, page, cx) = open_session(&user, &id, cx);

    until(&shell, cx, "both cards", |cx| {
        let open = questions(&page, cx);
        open.len() == 2 && open.iter().all(|(at, _)| card_shows(*at, cx))
    });
    let open = questions(&page, cx);
    assert_eq!(open[0].1, QUESTION);
    assert_eq!(open[1].1, SECOND);
    assert!(open[0].0 < open[1].0, "in order");
    let bell: Vec<String> = bell_of(&shell, &id, cx)
        .into_iter()
        .map(|(_, d)| d)
        .collect();
    assert_eq!(bell, [QUESTION, SECOND]);
    release(&user);
}

#[gpui_kit::test]
fn the_steps_line_of_ask_user_says_the_question(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "draw the charts");
    let (shell, page, cx) = open_session(&user, &id, cx);
    let asked = |b: &Block| matches!(b, Block::Steps { calls } if calls.iter().any(|c| c.name == "ask_user"));
    until(&shell, cx, "the call", |cx| {
        blocks(&page, cx).iter().any(asked)
    });
    let call = blocks(&page, cx)
        .into_iter()
        .find_map(|b| match b {
            Block::Steps { calls } => calls.into_iter().find(|c| c.name == "ask_user"),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        call.args,
        "asked you: **POC charts**: blocked by the minimum of 3 points?"
    );
    release(&user);
}

/// The card of the ask of row `at`.
fn card_of(
    page: &Entity<SessionPage>,
    at: usize,
    cx: &mut VisualTestContext,
) -> Entity<QuestionCard> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).rows()[at]
            .read(cx)
            .answer()
            .cloned()
    })
    .expect("the card's blocks")
}

/// Writes `text` in the field of question `n` of the card of row `at`.
fn write(page: &Entity<SessionPage>, at: usize, n: usize, text: &str, cx: &mut VisualTestContext) {
    let field = card_of(page, at, cx).read_with(cx, |card, _| card.field(n).clone());
    field.update_in(cx, |field, window, cx| field.set_value(text, window, cx));
    cx.run_until_parked();
}

/// Writes `text` in the note of question `n` of the card of row `at`.
fn note(page: &Entity<SessionPage>, at: usize, n: usize, text: &str, cx: &mut VisualTestContext) {
    let field = card_of(page, at, cx).read_with(cx, |card, _| card.note(n).clone());
    field.update_in(cx, |field, window, cx| field.set_value(text, window, cx));
    cx.run_until_parked();
}

/// Answers the single question of the card of row `at` with `text`.
fn answer(
    page: &Entity<SessionPage>,
    at: usize,
    text: &str,
    cx: &mut VisualTestContext,
) -> Entity<QuestionCard> {
    write(page, at, 0, text, cx);
    let card = card_of(page, at, cx);
    card.update(cx, |card, cx| card.send(cx));
    card
}

/// The text of every prompt the agent received.
fn prompts(user: &User) -> Vec<String> {
    user.received_requests("session/prompt")
        .iter()
        .map(|r| {
            r["params"]["prompt"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

#[gpui_kit::test]
fn two_asks_waiting_are_two_cards_answered_apart(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "ask twice");
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "both cards", |cx| {
        let open = questions(&page, cx);
        open.len() == 2 && open.iter().all(|(at, _)| card_shows(*at, cx))
    });
    settle(cx);
    let (first, second) = (questions(&page, cx)[0].0, questions(&page, cx)[1].0);

    answer(&page, first, "Drop it.", cx);
    release(&user);
    until(
        &shell,
        cx,
        "the first folds to its answer, the second still asks",
        |cx| {
            settle(cx);
            !has(format!("row-{first}-question-text"), cx)
                && has(format!("row-{first}-answered-0-answer"), cx)
        },
    );
    assert!(
        card_shows(second, cx),
        "the second waits for its own answer"
    );
    assert_eq!(mark_of(&shell, &id, cx), Some(AgentState::NeedsYou));
    let prompt = prompts(&user)
        .into_iter()
        .find(|p| p.contains("Drop it."))
        .expect("the answer reached the agent");
    assert!(
        !prompt.contains(SECOND),
        "it answers the first alone: {prompt}"
    );

    answer(&page, second, "Blue.", cx);
    until(&shell, cx, "the second folds to its answer", |cx| {
        settle(cx);
        !has(format!("row-{second}-question-text"), cx)
            && label(format!("row-{second}-answered-0-answer"), cx).as_deref() == Some("Blue.")
    });
}

/// A session that asked the three questions of #637, its card shown: the row of its card.
fn three_asked<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    String,
    Entity<Shell>,
    Entity<SessionPage>,
    usize,
    &'a mut VisualTestContext,
) {
    let id = session(user, "full", "ship it");
    let (shell, page, cx) = open_session(user, &id, cx);
    // Tall enough for the three questions' rows: a click needs its target in sight.
    cx.simulate_resize(size(px(1440.), px(1800.)));
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    (id, shell, page, at, cx)
}

fn click(id: String, cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.click(id, cx));
    cx.run_until_parked();
}

fn bounds(id: String, cx: &mut VisualTestContext) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update(|window, _| window.find(id).bounds())
}

/// Whether the field of the words of question `n` of the card of row `at` has the focus.
fn field_focused(
    page: &Entity<SessionPage>,
    at: usize,
    n: usize,
    cx: &mut VisualTestContext,
) -> bool {
    let field = card_of(page, at, cx).read_with(cx, |card, _| card.field(n).clone());
    cx.update(|window, cx| field.read(cx).focus_handle(cx).is_focused(window))
}

fn chosen(id: String, cx: &mut VisualTestContext) -> Option<bool> {
    cx.update(|window, _| window.try_find(id).and_then(|found| found.selected()))
}

/// Answers the three questions: Yes with a note, a title, Alice and Carol.
fn answer_three(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) {
    click(format!("row-{at}-question-0-option-0"), cx);
    settle(cx);
    click(format!("row-{at}-note-0-add"), cx);
    settle(cx);
    note(page, at, 0, "Squash it first.", cx);
    write(page, at, 1, "Export orders as CSV", cx);
    click(format!("row-{at}-question-2-option-0"), cx);
    click(format!("row-{at}-question-2-option-2"), cx);
    settle(cx);
    click(format!("row-{at}-answer-send"), cx);
}

#[gpui_kit::test]
fn the_question_card_names_the_session_that_asks(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "draw the charts");
    user.call(
        "agent.rename",
        json!({"agent": id, "title": "Charts of the quarter"}),
    )
    .unwrap();
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "the card, named after the session", |cx| {
        questions(&page, cx).first().is_some_and(|(at, _)| {
            card_shows(*at, cx)
                && label(format!("row-{at}-question-title"), cx).as_deref()
                    == Some("Charts of the quarter")
        })
    });
    let at = questions(&page, cx)[0].0;
    assert_eq!(
        label(format!("row-{at}-question-tag"), cx).as_deref(),
        Some("asks you")
    );
    let shown = cx.update(|window, _| window.try_find_by_label("the conversation").is_some());
    assert!(!shown, "never the conversation");
    release(&user);
}

#[gpui_kit::test]
fn an_ask_with_three_questions_shows_three_blocks_each_answered_apart(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    assert_eq!(
        questions(&page, cx)[0].1.lines().count(),
        5,
        "one ask, its text numbered"
    );
    for n in 0..3 {
        assert!(has(format!("row-{at}-question-{n}"), cx), "block {n}");
    }
    assert!(has(format!("row-{at}-question-0-options"), cx));
    assert!(
        !has(format!("row-{at}-question-1-options"), cx),
        "a question in words alone"
    );
    assert!(
        has(format!("row-{at}-answer-1-frame"), cx),
        "its field is there at once"
    );
    assert_eq!(
        label(format!("row-{at}-answer-1-field-label"), cx).as_deref(),
        Some("Your answer"),
        "a question in words alone keeps its labelled field"
    );
    assert!(
        !has(format!("row-{at}-answer-0-frame"), cx),
        "the words of a question of options come with Something else"
    );
    assert!(has(format!("row-{at}-question-2-option-2"), cx));

    // One choice: the second replaces the first. Several: both stay.
    click(format!("row-{at}-question-0-option-0"), cx);
    click(format!("row-{at}-question-0-option-1"), cx);
    click(format!("row-{at}-question-2-option-0"), cx);
    click(format!("row-{at}-question-2-option-1"), cx);
    settle(cx);
    assert_eq!(
        chosen(format!("row-{at}-question-0-option-0"), cx),
        Some(false)
    );
    assert_eq!(
        chosen(format!("row-{at}-question-0-option-1"), cx),
        Some(true)
    );
    assert_eq!(
        chosen(format!("row-{at}-question-2-option-0"), cx),
        Some(true)
    );
    assert_eq!(
        chosen(format!("row-{at}-question-2-option-1"), cx),
        Some(true)
    );
    // Words written in one field stay in it.
    write(&page, at, 1, "Export orders as CSV", cx);
    let answers = card_of(&page, at, cx).read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[0].text, "");
    assert_eq!(answers[0].choices, ["Not yet"]);
    assert_eq!(answers[1].text, "Export orders as CSV");
    assert_eq!(answers[2].choices, ["Alice", "Bob"]);
    at_rest(cx);
}

#[gpui_kit::test]
fn an_option_is_a_row_with_its_description_under_it(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, _, at, cx) = three_asked(&user, cx);
    let row = format!("row-{at}-question-0-option-1");
    assert_eq!(label(row.clone(), cx).as_deref(), Some("Not yet"));
    assert_eq!(
        label(format!("{row}-line"), cx).as_deref(),
        Some("Keep it local")
    );
    let (row, words, line) = cx.update(|window, _| {
        (
            window.find(row.clone()).bounds(),
            window.find(format!("{row}-label")).bounds(),
            window.find(format!("{row}-line")).bounds(),
        )
    });
    assert!(words.bottom() <= line.top(), "under its label");
    assert!(line.bottom() <= row.bottom());
    // The last row of each question is the way out, in its own words.
    assert_eq!(
        label(format!("row-{at}-question-0-other"), cx).as_deref(),
        Some("Something else…")
    );
    assert_eq!(
        label(format!("row-{at}-question-2-other"), cx).as_deref(),
        Some("Something else…")
    );
}

#[gpui_kit::test]
fn a_long_option_wraps_and_stays_in_the_card(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "long options");
    let (shell, page, cx) = open_session(&user, &id, cx);
    cx.simulate_resize(size(px(760.), px(800.)));
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| has(format!("row-{at}-question-0-option-0-line"), cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let (card, line) = cx.update(|window, _| {
        (
            window.find(format!("row-{at}-question-text")).bounds(),
            window
                .find(format!("row-{at}-question-0-option-0-line"))
                .bounds(),
        )
    });
    assert!(line.right() <= card.right(), "{line:?} in {card:?}");
    assert!(line.size.height > px(36.), "on several lines: {line:?}");
}

/// The three questions answered in long words: a choice with a long unbroken note, a long
/// spaced answer, and the choices of the third left as they were.
fn answer_three_at_length(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) {
    click(format!("row-{at}-question-0-option-0"), cx);
    settle(cx);
    click(format!("row-{at}-note-0-add"), cx);
    settle(cx);
    note(page, at, 0, guard::UNBROKEN, cx);
    write(page, at, 1, guard::SPACED, cx);
    click(format!("row-{at}-question-2-option-0"), cx);
    click(format!("row-{at}-question-2-option-2"), cx);
    settle(cx);
    click(format!("row-{at}-answer-send"), cx);
}

#[gpui_kit::test]
fn a_long_answer_stays_inside_its_answered_card(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = three_asked(&user, cx);
    guard::narrow(cx);
    answer_three_at_length(&page, at, cx);
    until(&shell, cx, "the card folds to its answers", |cx| {
        settle(cx);
        !has(format!("row-{at}-question-text"), cx)
            && has(format!("row-{at}-answered-2-answer"), cx)
    });
    settle(cx);
    guard::assert_stays_inside(cx, format!("row-{at}-question"));
    // The words are the user's own: they wrap, whole.
    let (note, spaced) = cx.update(|window, _| {
        (
            window.find(format!("row-{at}-answered-0-note")).bounds(),
            window
                .find(format!("row-{at}-answered-1-headline"))
                .bounds(),
        )
    });
    assert!(spaced.size.height > px(36.), "wrapped on lines: {spaced:?}");
    assert!(
        note.size.height > px(36.),
        "the long word wraps too: {note:?}"
    );
}

#[gpui_kit::test]
fn a_long_question_stays_inside_its_open_card(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "long ask");
    let (shell, page, cx) = open_session(&user, &id, cx);
    guard::narrow(cx);
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    guard::assert_stays_inside(cx, format!("row-{at}-question"));
}

#[gpui_kit::test]
fn several_choices_have_square_marks_and_say_several_once(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, _, at, cx) = three_asked(&user, cx);
    assert!(has(format!("row-{at}-question-2-option-0-tick"), cx));
    assert!(!has(format!("row-{at}-question-2-option-0-dot"), cx));
    assert!(has(format!("row-{at}-question-0-option-0-dot"), cx));
    assert!(!has(format!("row-{at}-question-0-option-0-tick"), cx));
    assert_eq!(
        label(format!("row-{at}-question-2-several"), cx).as_deref(),
        Some("several")
    );
    assert!(!has(format!("row-{at}-question-0-several"), cx));
    let said = cx.update(|window, _| window.find_all_by_label("several").len());
    assert_eq!(said, 1, "said once");
}

#[gpui_kit::test]
fn something_else_holds_a_named_field_and_needs_words(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let card = card_of(&page, at, cx);
    click(format!("row-{at}-question-0-option-0"), cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("row-{at}-question-2-option-0"), cx);
    settle(cx);
    assert!(card.read_with(cx, |card, cx| card.is_ready(cx)));
    assert!(!has(format!("row-{at}-answer-0-frame"), cx), "folded");

    // Picked, it takes the place of the choice and holds its field, named.
    click(format!("row-{at}-question-0-other"), cx);
    settle(cx);
    assert_eq!(
        chosen(format!("row-{at}-question-0-option-0"), cx),
        Some(false)
    );
    assert_eq!(chosen(format!("row-{at}-question-0-other"), cx), Some(true));
    assert!(has(format!("row-{at}-answer-0-frame"), cx));
    assert_eq!(
        label(format!("row-{at}-answer-0-frame"), cx).as_deref(),
        Some("Your answer")
    );
    assert!(
        !card.read_with(cx, |card, cx| card.is_ready(cx)),
        "it needs its words"
    );
    write(&page, at, 0, "Both, in that order", cx);
    assert!(card.read_with(cx, |card, cx| card.is_ready(cx)));
    let answers = card.read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[0].choices, Vec::<String>::new());
    assert_eq!(answers[0].text, "Both, in that order");

    // Let go of, from its words (the middle of the row is its field), it folds away.
    click(format!("row-{at}-question-0-other-label"), cx);
    settle(cx);
    assert!(!has(format!("row-{at}-answer-0-frame"), cx), "folded again");
    at_rest(cx);
}

#[gpui_kit::test]
fn the_field_of_something_else_is_in_its_row_right_of_the_mark_and_focused(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let other = format!("row-{at}-question-0-other");
    assert!(
        !has(format!("{other}-field"), cx),
        "not picked: a row as any"
    );
    assert!(!field_focused(&page, at, 0, cx));
    let alone = bounds(other.clone(), cx).size.height;

    click(other.clone(), cx);
    settle(cx);
    let (row, mark, words, field) = (
        bounds(other.clone(), cx),
        bounds(format!("{other}-mark"), cx),
        bounds(format!("{other}-label"), cx),
        bounds(format!("row-{at}-answer-0-frame"), cx),
    );
    assert!(
        row.contains(&field.origin) && field.right() <= row.right(),
        "the field is inside the row: {row:?} {field:?}"
    );
    assert!(
        mark.right() <= words.left() && words.right() <= field.left(),
        "the mark, then the words, then the field: {mark:?} {words:?} {field:?}"
    );
    assert!(row.size.height > alone, "the row is as high as its field");
    assert_eq!(
        label(format!("{other}-label"), cx).as_deref(),
        Some("Something else:"),
        "its words stay beside the field"
    );
    assert!(field_focused(&page, at, 0, cx), "focused at once");
    at_rest(cx);
}

#[gpui_kit::test]
fn ctrl_enter_in_the_field_of_something_else_sends(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = three_asked(&user, cx);
    let card = card_of(&page, at, cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("row-{at}-question-2-option-0"), cx);
    click(format!("row-{at}-question-0-other"), cx);
    settle(cx);
    assert!(field_focused(&page, at, 0, cx));
    cx.simulate_input("Both, in that order");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("and say why");
    cx.run_until_parked();
    assert_eq!(
        card.read_with(cx, |card, cx| card.field(0).read(cx).value().to_string()),
        "Both, in that order\nand say why",
        "Enter breaks the line in the row's field"
    );
    assert!(!card.read_with(cx, |card, cx| card.is_sent(cx)));
    cx.simulate_keystrokes("ctrl-enter");
    until(&shell, cx, "the answer went", |cx| {
        card.read_with(cx, |card, cx| card.is_sent(cx))
    });
    settle(cx);
    assert!(
        !has(format!("row-{at}-answer-0-frame"), cx),
        "the field gave way once sent from the field itself"
    );
    assert!(!field_focused(&page, at, 0, cx), "and holds no focus");
}

#[gpui_kit::test]
fn the_field_in_the_row_grows_to_several_lines_inside_it(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let (other, frame) = (
        format!("row-{at}-question-0-other"),
        format!("row-{at}-answer-0-frame"),
    );
    click(other.clone(), cx);
    settle(cx);
    let one = (bounds(other.clone(), cx), bounds(frame.clone(), cx));
    write(&page, at, 0, "First line\nSecond line\nThird line", cx);
    settle(cx);
    let three = (bounds(other.clone(), cx), bounds(frame.clone(), cx));
    assert!(
        three.1.size.height > one.1.size.height + px(30.),
        "the field grows with its lines: {:?} {:?}",
        one.1,
        three.1
    );
    assert!(
        three.0.size.height > one.0.size.height + px(30.),
        "so does the row"
    );
    assert!(
        three.0.contains(&three.1.origin) && three.1.bottom() <= three.0.bottom(),
        "the field stays inside the row"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn in_a_question_of_several_choices_something_else_holds_its_field_too(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let other = format!("row-{at}-question-2-other");
    click(format!("row-{at}-question-2-option-0"), cx);
    click(other.clone(), cx);
    settle(cx);
    assert!(has(format!("{other}-tick"), cx), "a square mark");
    assert_eq!(chosen(other.clone(), cx), Some(true));
    assert_eq!(
        chosen(format!("row-{at}-question-2-option-0"), cx),
        Some(true),
        "the other choice stays"
    );
    let (row, mark, field) = (
        bounds(other.clone(), cx),
        bounds(format!("{other}-mark"), cx),
        bounds(format!("row-{at}-answer-2-frame"), cx),
    );
    assert!(
        row.contains(&field.origin) && mark.right() <= field.left(),
        "the field is in the row, right of the mark: {row:?} {field:?}"
    );
    assert!(field_focused(&page, at, 2, cx), "focused at once");
    at_rest(cx);
}

#[gpui_kit::test]
fn the_field_of_something_else_gives_way_once_the_answer_is_sent(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = three_asked(&user, cx);
    let card = card_of(&page, at, cx);
    click(format!("row-{at}-question-0-other"), cx);
    write(&page, at, 0, "Both, in that order", cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("row-{at}-question-2-option-0"), cx);
    settle(cx);
    assert!(has(format!("row-{at}-answer-0-frame"), cx));
    click(format!("row-{at}-answer-send"), cx);
    until(&shell, cx, "the answer went", |cx| {
        card.read_with(cx, |card, cx| card.is_sent(cx))
    });
    settle(cx);
    assert!(
        !has(format!("row-{at}-answer-0-frame"), cx),
        "the field of the row folded away, no longer editable"
    );
    assert!(!has(format!("row-{at}-answer-1-frame"), cx));
    assert!(
        !field_focused(&page, at, 0, cx),
        "nothing is typed into a field that gave way"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn nothing_under_the_options_holds_the_words_of_something_else(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, _, at, cx) = three_asked(&user, cx);
    click(format!("row-{at}-question-0-other"), cx);
    click(format!("row-{at}-question-2-other"), cx);
    settle(cx);
    assert!(has(format!("row-{at}-answer-0-frame"), cx));
    for n in [0, 2] {
        assert!(
            !has(format!("row-{at}-answer-{n}-field"), cx),
            "no separate field under the options of question {n}"
        );
        assert!(!has(format!("row-{at}-answer-{n}-field-label"), cx));
        assert!(!has(format!("row-{at}-question-{n}-words"), cx));
    }
}

#[gpui_kit::test]
fn letting_go_of_something_else_folds_its_field_and_keeps_the_words(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let other = format!("row-{at}-question-0-other");
    let alone = bounds(other.clone(), cx).size.height;
    click(other.clone(), cx);
    settle(cx);
    write(&page, at, 0, "Both, in that order", cx);

    click(format!("{other}-label"), cx);
    settle(cx);
    assert!(!has(format!("row-{at}-answer-0-frame"), cx), "folded away");
    assert_eq!(
        bounds(other.clone(), cx).size.height,
        alone,
        "the row as before"
    );
    assert_eq!(
        label(format!("{other}-label"), cx).as_deref(),
        Some("Something else…"),
        "the label is back"
    );
    assert!(!field_focused(&page, at, 0, cx));
    let card = card_of(&page, at, cx);
    assert!(
        cx.update(|window, cx| card.read(cx).has_focus(window)),
        "the focus goes back to the card"
    );

    click(other, cx);
    settle(cx);
    assert!(has(format!("row-{at}-answer-0-frame"), cx));
    let words = card_of(&page, at, cx).read_with(cx, |card, cx| card.field(0).read(cx).value());
    assert_eq!(words.as_ref(), "Both, in that order", "kept as it was");
    at_rest(cx);
}

#[gpui_kit::test]
fn add_a_note_appears_after_a_choice_and_sends_the_note(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = three_asked(&user, cx);
    assert!(!has(format!("row-{at}-note-0-add"), cx), "no choice yet");
    click(format!("row-{at}-question-0-option-0"), cx);
    settle(cx);
    assert_eq!(
        label(format!("row-{at}-note-0-add"), cx).as_deref(),
        Some("+ Add a note")
    );
    assert!(!has(format!("row-{at}-note-0-frame"), cx));
    click(format!("row-{at}-note-0-add"), cx);
    settle(cx);
    assert!(has(format!("row-{at}-note-0-frame"), cx), "its field");
    assert_eq!(
        label(format!("row-{at}-note-0-field-label"), cx).as_deref(),
        Some("Note with your choice")
    );
    assert!(!has(format!("row-{at}-note-0-add"), cx), "the button left");
    note(&page, at, 0, "Squash it first.", cx);
    let answers = card_of(&page, at, cx).read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[0].choices, ["Yes"]);
    assert_eq!(answers[0].text, "Squash it first.");

    // The note goes with the choice.
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("row-{at}-question-2-option-1"), cx);
    settle(cx);
    click(format!("row-{at}-answer-send"), cx);
    until(&shell, cx, "the agent took it", |_| {
        prompts(&user)
            .iter()
            .any(|p| p.contains("Yes\nSquash it first."))
    });
}

#[gpui_kit::test]
fn answer_waits_until_every_question_is_answered_and_says_why(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = three_asked(&user, cx);
    let card = card_of(&page, at, cx);
    let why = format!("row-{at}-answer-why");
    assert_eq!(
        label(why.clone(), cx).as_deref(),
        Some("Answer each question to send.")
    );
    click(format!("row-{at}-answer-send"), cx);
    settle(cx);
    assert!(
        !card.read_with(cx, |card, cx| card.is_sent(cx)),
        "Answer waits"
    );
    click(format!("row-{at}-question-0-option-0"), cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("row-{at}-question-2-option-1"), cx);
    settle(cx);
    assert_eq!(
        label(why, cx).as_deref(),
        Some("Sent to the agent as one message.")
    );
    assert_eq!(
        label(format!("row-{at}-answer-keys"), cx).as_deref(),
        Some("Ctrl+Enter")
    );
    click(format!("row-{at}-answer-send"), cx);
    until(&shell, cx, "the answer went", |cx| {
        card.read_with(cx, |card, cx| card.is_sent(cx))
    });
}

#[gpui_kit::test]
fn the_count_says_how_many_are_answered(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let count = format!("row-{at}-question-count");
    assert_eq!(label(count.clone(), cx).as_deref(), Some("0 of 3 answered"));
    click(format!("row-{at}-question-0-option-0"), cx);
    settle(cx);
    assert_eq!(label(count.clone(), cx).as_deref(), Some("1 of 3 answered"));
    write(&page, at, 1, "Export orders as CSV", cx);
    settle(cx);
    assert_eq!(label(count.clone(), cx).as_deref(), Some("2 of 3 answered"));
    // Something else, with no words yet, is not an answer.
    click(format!("row-{at}-question-2-other"), cx);
    settle(cx);
    assert_eq!(label(count.clone(), cx).as_deref(), Some("2 of 3 answered"));
    write(&page, at, 2, "Whoever is free", cx);
    settle(cx);
    assert_eq!(label(count, cx).as_deref(), Some("3 of 3 answered"));
    at_rest(cx);
}

#[gpui_kit::test]
fn an_option_chosen_and_words_written_go_back_together(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, _, at, cx) = three_asked(&user, cx);
    let page = session_page(&shell, cx);
    answer_three(&page, at, cx);
    until(&shell, cx, "the agent took the answer", |_| {
        prompts(&user)
            .iter()
            .any(|p| p.contains("Export orders as CSV"))
    });
    let prompt = prompts(&user)
        .into_iter()
        .find(|p| p.contains("Export orders as CSV"))
        .unwrap();
    assert!(
        prompt.contains(&format!("> {PUSH}\n\nYes\nSquash it first.")),
        "the choice and the words together: {prompt}"
    );
}

#[gpui_kit::test]
fn the_answer_message_quotes_each_question_and_its_answer(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, _, at, cx) = three_asked(&user, cx);
    let page = session_page(&shell, cx);
    answer_three(&page, at, cx);
    until(&shell, cx, "the agent took the answer", |_| {
        prompts(&user)
            .iter()
            .any(|p| p.contains("Export orders as CSV"))
    });
    let prompt = prompts(&user)
        .into_iter()
        .find(|p| p.contains("Export orders as CSV"))
        .unwrap();
    assert!(
        prompt.contains(&format!(
            "> {PUSH}\n\nYes\nSquash it first.\n\n> {TITLE}\n\nExport orders as CSV\n\n\
             > {REVIEWS}\n\nAlice, Carol"
        )),
        "{prompt}"
    );
}

#[gpui_kit::test]
fn answer_is_disabled_until_every_question_has_an_answer(cx: &mut TestAppContext) {
    let user = user();
    let (id, shell, page, at, cx) = three_asked(&user, cx);
    let card = card_of(&page, at, cx);
    click(format!("row-{at}-question-0-option-0"), cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    assert!(
        !card.read_with(cx, |card, cx| card.is_ready(cx)),
        "the third has no answer"
    );
    click(format!("row-{at}-answer-send"), cx);
    settle(cx);
    assert!(
        !card.read_with(cx, |card, cx| card.is_sent(cx)),
        "nothing sent"
    );
    // Words alone answer a question of options too, through Something else.
    click(format!("row-{at}-question-2-other"), cx);
    write(&page, at, 2, "Whoever is free", cx);
    assert!(card.read_with(cx, |card, cx| card.is_ready(cx)));
    click(format!("row-{at}-answer-send"), cx);
    until(&shell, cx, "the answer went", |cx| {
        card.read_with(cx, |card, cx| card.is_sent(cx))
    });
    let _ = id;
}

#[gpui_kit::test]
fn enter_breaks_a_line_in_an_answer_and_ctrl_enter_sends(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "draw the charts");
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let card = card_of(&page, at, cx);
    let field = card.read_with(cx, |card, _| card.field(0).clone());
    field.update_in(cx, |field, window, cx| field.focus(window, cx));
    cx.simulate_input("Drop it");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("and say why");
    cx.run_until_parked();
    assert_eq!(
        field.read_with(cx, |field, _| field.value().to_string()),
        "Drop it\nand say why",
        "Enter breaks the line"
    );
    assert!(
        !card.read_with(cx, |card, cx| card.is_sent(cx)),
        "Enter sends nothing"
    );
    cx.simulate_keystrokes("ctrl-enter");
    until(&shell, cx, "the answer went", |cx| {
        card.read_with(cx, |card, cx| card.is_sent(cx))
    });
    release(&user);
    until(&shell, cx, "the agent took it", |_| {
        prompts(&user)
            .iter()
            .any(|p| p.contains("Drop it\nand say why"))
    });
}

#[gpui_kit::test]
fn a_single_question_ask_still_works_as_before(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "draw the charts");
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    assert!(has(format!("row-{at}-question-0"), cx));
    assert!(!has(format!("row-{at}-question-1"), cx), "one block");
    assert!(
        !has(format!("row-{at}-question-0-options"), cx),
        "no options"
    );
    answer(&page, at, "Drop it.", cx);
    release(&user);
    until(&shell, cx, "the agent took it", |_| {
        prompts(&user).iter().any(|p| p.contains("Drop it."))
    });
    let prompt = prompts(&user)
        .into_iter()
        .find(|p| p.contains("Drop it."))
        .unwrap();
    assert!(
        !prompt.contains("> "),
        "the words alone, as before: {prompt}"
    );
}

#[gpui_kit::test]
fn the_card_folds_to_its_answers_with_the_notes(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = three_asked(&user, cx);
    answer_three(&page, at, cx);
    until(&shell, cx, "the card folds to its answers", |cx| {
        settle(cx);
        !has(format!("row-{at}-question-text"), cx)
            && has(format!("row-{at}-answered-2-answer"), cx)
    });
    assert_eq!(
        label(format!("row-{at}-answered-0-question"), cx).as_deref(),
        Some(PUSH)
    );
    assert_eq!(
        label(format!("row-{at}-answered-0-answer"), cx).as_deref(),
        Some("Yes\nSquash it first.")
    );
    assert_eq!(
        label(format!("row-{at}-answered-1-answer"), cx).as_deref(),
        Some("Export orders as CSV")
    );
    assert_eq!(
        label(format!("row-{at}-answered-2-answer"), cx).as_deref(),
        Some("Alice, Carol")
    );
    assert!(label(format!("row-{at}-answered-title"), cx).is_some());
    assert_eq!(
        label(format!("row-{at}-answered-tag"), cx).as_deref(),
        Some("answered")
    );
    // No bubble of the user repeats the quoted text.
    let quoted = blocks(&page, cx)
        .into_iter()
        .any(|b| matches!(b, Block::Message { text, .. } if text.contains("> ") || text.contains("Export orders")));
    assert!(!quoted, "{:?}", blocks(&page, cx));
    at_rest(cx);
}

#[gpui_kit::test]
fn an_answer_sent_stays_sent_when_the_page_is_opened_again(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "draw the charts");
    let other = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
    });
    let at = questions(&page, cx)[0].0;
    let sent = answer(&page, at, "Drop it.", cx);
    until(&shell, cx, "the answer went", |cx| {
        sent.read_with(cx, |a, cx| a.is_sent(cx))
    });

    // Another session, then this one again: a new page, whose card still knows.
    cx.update(|window, cx| window.click(session_id(&other), cx));
    until(&shell, cx, "the other page", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.id() == other)
    });
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "this page again, its card", |cx| {
        let page = session_page(&shell, cx);
        page.read_with(cx, |p, _| p.id() == id)
            && questions(&page, cx)
                .first()
                .is_some_and(|(at, _)| has(format!("row-{at}-question-text"), cx))
    });
    settle(cx);
    let page = session_page(&shell, cx);
    let at = questions(&page, cx)[0].0;
    assert_eq!(
        label(format!("row-{at}-answer-waits"), cx).as_deref(),
        Some(ANSWER_WAITS)
    );
    assert!(
        !has(format!("row-{at}-answer-0-frame"), cx),
        "no field to answer again"
    );
    release(&user);
}

#[gpui_kit::test]
fn a_card_answered_before_a_restart_stays_answered(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "draw the charts");
    let other = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
    });
    let at = questions(&page, cx)[0].0;
    let sent = answer(&page, at, "Drop it.", cx);
    until(&shell, cx, "the answer went", |cx| {
        sent.read_with(cx, |a, cx| a.is_sent(cx))
    });

    // The window starts again while the answer waits: only the daemon's history is left.
    cx.update(|_, cx| flottille_window::question_card::forget_sent_answers(cx));
    cx.update(|window, cx| window.click(session_id(&other), cx));
    until(&shell, cx, "the other page", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.id() == other)
    });
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "this page again, its card", |cx| {
        let page = session_page(&shell, cx);
        page.read_with(cx, |p, _| p.id() == id)
            && questions(&page, cx)
                .first()
                .is_some_and(|(at, _)| has(format!("row-{at}-question-text"), cx))
    });
    settle(cx);
    let page = session_page(&shell, cx);
    let at = questions(&page, cx)[0].0;
    assert_eq!(
        label(format!("row-{at}-answer-waits"), cx).as_deref(),
        Some(ANSWER_WAITS)
    );
    assert!(
        !has(format!("row-{at}-answer-0-frame"), cx),
        "no field to answer again"
    );
    release(&user);
}

#[gpui_kit::test]
fn ctrl_enter_answers_a_card_of_options_alone(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "pick one");
    let (shell, page, cx) = open_session(&user, &id, cx);
    cx.simulate_resize(size(px(1440.), px(1800.)));
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(*at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let card = card_of(&page, at, cx);

    // No field is open: the card has only rows, and a click on one is all the user did.
    click(format!("row-{at}-question-0-option-0"), cx);
    settle(cx);
    assert!(card.read_with(cx, |card, cx| card.is_ready(cx)));
    assert!(!card.read_with(cx, |card, cx| card.is_sent(cx)));
    cx.simulate_keystrokes("ctrl-enter");
    until(&shell, cx, "the answer went", |cx| {
        card.read_with(cx, |card, cx| card.is_sent(cx))
    });
}

#[gpui_kit::test]
fn something_else_needs_words_even_with_another_choice(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let card = card_of(&page, at, cx);
    click(format!("row-{at}-question-0-option-0"), cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("row-{at}-question-2-option-0"), cx);
    settle(cx);
    assert!(card.read_with(cx, |card, cx| card.is_ready(cx)));

    // A question of several choices keeps Alice, and "Something else…" is picked beside her.
    click(format!("row-{at}-question-2-other"), cx);
    settle(cx);
    assert_eq!(
        chosen(format!("row-{at}-question-2-option-0"), cx),
        Some(true)
    );
    assert_eq!(chosen(format!("row-{at}-question-2-other"), cx), Some(true));
    assert!(
        !card.read_with(cx, |card, cx| card.is_ready(cx)),
        "it needs its words"
    );
    assert_eq!(card.read_with(cx, |card, cx| card.answered_count(cx)), 2);
    assert_eq!(
        label(format!("row-{at}-question-count"), cx).as_deref(),
        Some("2 of 3 answered")
    );
    click(format!("row-{at}-answer-send"), cx);
    settle(cx);
    assert!(
        !card.read_with(cx, |card, cx| card.is_sent(cx)),
        "nothing sent"
    );

    write(&page, at, 2, "Whoever is free", cx);
    assert!(card.read_with(cx, |card, cx| card.is_ready(cx)));
    let answers = card.read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[2].choices, ["Alice"]);
    assert_eq!(answers[2].text, "Whoever is free");
}

#[gpui_kit::test]
fn letting_the_choice_go_folds_and_clears_the_note(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    click(format!("row-{at}-question-0-option-0"), cx);
    settle(cx);
    click(format!("row-{at}-note-0-add"), cx);
    settle(cx);
    note(&page, at, 0, "Squash it first.", cx);
    assert!(has(format!("row-{at}-note-0-frame"), cx));

    // The choice goes, and the note with it.
    click(format!("row-{at}-question-0-option-0"), cx);
    settle(cx);
    assert!(!has(format!("row-{at}-note-0-frame"), cx), "folded");
    assert!(
        !has(format!("row-{at}-note-0-add"), cx),
        "no choice to note"
    );
    let card = card_of(&page, at, cx);
    let kept = card.read_with(cx, |card, cx| card.note(0).read(cx).value().to_string());
    assert_eq!(kept, "", "emptied");

    // Choosing again offers "+ Add a note" afresh, with nothing written.
    click(format!("row-{at}-question-0-option-1"), cx);
    settle(cx);
    assert_eq!(
        label(format!("row-{at}-note-0-add"), cx).as_deref(),
        Some("+ Add a note")
    );
    assert!(!has(format!("row-{at}-note-0-frame"), cx));
    let answers = card.read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[0].text, "");
}
