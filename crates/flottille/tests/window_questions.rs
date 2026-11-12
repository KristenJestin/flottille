//! The scenarios of #519 in the window: a question a root asks the user with `ask_user` in the
//! middle of its turn shows at once, as a card docked above the composer with the field that
//! answers it and a line at its place in the conversation (#777), in the bell and as "needs you"
//! on the session's row, until its answer is taken. On GPUI's test platform in front of a real daemon whose agents are the fake ACP agent,
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
const FOUR: [&str; 4] = [
    "Which database?",
    "Which cache?",
    "Which regions?",
    "Anything else?",
];
const LONG_OPTION: &str = "You drop me a bundle or a clone of the real repository, and I replay \
    its commits one by one so that the history stays whole in the new place, with every author \
    and every date as they were.";

/// The agent asks, goes on with a call and a sentence, then works until the test lets it end.
fn script() -> Value {
    let ask = |q: &str| json!({"mcp": {"tool": "ask_user", "arguments": {"question": q}}});
    // Longer than the conversation's height: it scrolls (#777).
    let long_look = (1..=40)
        .map(|n| format!("Line {n} of a long look at the code."))
        .collect::<Vec<_>>()
        .join("\n\n");
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
        {"when": "ask four", "do": [
            {"say": long_look},
            {"mcp": {"tool": "ask", "arguments": {"questions": [
                {"question": FOUR[0], "options": [{"label": "Postgres"}, {"label": "SQLite"}]},
                {"question": FOUR[1], "options": [{"label": "Redis"}, {"label": "None"}]},
                {"question": FOUR[2], "multiple": true,
                 "options": [{"label": "Europe"}, {"label": "America"}, {"label": "Asia"}]},
                {"question": FOUR[3]}
            ]}}},
            {"say": "Asked four. Meanwhile, a long look at the code follows."},
            {"say": "Still looking."}]},
        {"when": "ask beside a helper", "do": [
            {"mcp": {"tool": "agent_launch",
                "arguments": {"kind": "child", "title": "Helper", "brief": "help me look"}}},
            {"mcp": {"tool": "ask", "arguments": {"questions": [
                {"question": FOUR[0], "options": [{"label": "Postgres"}, {"label": "SQLite"}]},
                {"question": FOUR[1], "options": [{"label": "Redis"}, {"label": "None"}]}
            ]}}},
            {"say": "Asked."}]},
        {"when": "help me look", "do": [{"hang": true}]},
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
    if let gpui_kit::ElementId::Name(name) = &id
        && !name.ends_with("-question-step")
    {
        to_question_of(name, cx);
    }
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

/// The number of the ask of row `at`, which names the elements of its card.
fn seq_of(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) -> u64 {
    match &blocks(page, cx)[at] {
        Block::Question { seq, .. } => *seq,
        other => panic!("no ask at row {at}: {other:?}"),
    }
}

/// Whether the card of the question of row `at` shows, with its first question.
fn card_shows(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) -> bool {
    let ask = seq_of(page, at, cx);
    has(format!("ask-{ask}-question-text"), cx) && has(format!("ask-{ask}-question-0-text"), cx)
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
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
    let ask = seq_of(&page, at, cx);
    assert_eq!(text, QUESTION, "the question in full");
    assert!(
        card_shows(&page, at, cx),
        "the card stays while the agent works"
    );
    assert_eq!(
        label(format!("ask-{ask}-answer-send"), cx).as_deref(),
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
        card_shows(&page, at, cx),
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    let at = questions(&page, cx)[0].0;
    let ask = seq_of(&page, at, cx);
    // Answered while the agent works: it waits for the end of the turn, and says so.
    let answer = answer(&page, at, "Drop it.", cx);
    until(&shell, cx, "the answer went", |cx| {
        answer.read_with(cx, |a, cx| a.is_sent(cx))
    });
    settle(cx);
    assert_eq!(
        label(format!("ask-{ask}-answer-waits"), cx).as_deref(),
        Some(ANSWER_WAITS)
    );
    assert!(has(format!("ask-{ask}-question-text"), cx), "still open");

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
        !has(format!("ask-{ask}-question-text"), cx)
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

    // Both lines in the conversation, the oldest docked.
    until(&shell, cx, "both asks", |cx| {
        let open = questions(&page, cx);
        open.len() == 2
            && open
                .iter()
                .all(|(at, _)| has(format!("row-{at}-question-line"), cx))
            && card_shows(&page, open[0].0, cx)
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

/// The card of the ask of row `at`, in the dock.
fn card_of(
    page: &Entity<SessionPage>,
    at: usize,
    cx: &mut VisualTestContext,
) -> Entity<QuestionCard> {
    let ask = seq_of(page, at, cx);
    page.read_with(cx, |page, cx| {
        page.ask_dock().read(cx).card(ask, cx).cloned()
    })
    .expect("the card in the dock")
}

/// The card shown in the dock.
fn docked(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> Option<Entity<QuestionCard>> {
    page.read_with(cx, |page, cx| page.ask_dock().read(cx).shown().cloned())
}

/// The question an element of a card belongs to, from its id: `ask-<seq>-question-<n>-…`,
/// `ask-<seq>-note-<n>-…`, `ask-<seq>-answer-<n>-…`.
fn question_of(id: &str) -> Option<(u64, usize)> {
    let rest = id.strip_prefix("ask-")?;
    let (ask, rest) = rest.split_once('-')?;
    let ask = ask.parse().ok()?;
    ["question-", "note-", "answer-"].iter().find_map(|part| {
        let n = rest.strip_prefix(part)?;
        let n = n.split('-').next()?.parse().ok()?;
        Some((ask, n))
    })
}

/// Shows question `n` of the card of the ask `ask`, as the user does with Previous and Next.
fn show(ask: u64, n: usize, cx: &mut VisualTestContext) {
    to_question_of(&format!("ask-{ask}-question-{n}-"), cx);
}

/// Goes, as the user does with Previous and Next, to the question `id` belongs to.
fn to_question_of(id: &str, cx: &mut VisualTestContext) {
    let Some((ask, n)) = question_of(id) else {
        return;
    };
    for _ in 0..4 {
        let Some(step) = label(format!("ask-{ask}-question-step"), cx) else {
            return;
        };
        let current: usize = step.split(' ').next().unwrap().parse().unwrap();
        let button = match (current - 1).cmp(&n) {
            std::cmp::Ordering::Equal => return,
            std::cmp::Ordering::Less => "next",
            std::cmp::Ordering::Greater => "previous",
        };
        cx.update(|window, cx| window.click(format!("ask-{ask}-question-{button}"), cx));
        cx.run_until_parked();
        settle(cx);
    }
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
    until(&shell, cx, "both asks, the first docked", |cx| {
        let open = questions(&page, cx);
        open.len() == 2 && card_shows(&page, open[0].0, cx)
    });
    settle(cx);
    let (first, second) = (questions(&page, cx)[0].0, questions(&page, cx)[1].0);
    let (first_ask, second_ask) = (seq_of(&page, first, cx), seq_of(&page, second, cx));

    answer(&page, first, "Drop it.", cx);
    release(&user);
    until(
        &shell,
        cx,
        "the first folds to its answer, the second still asks",
        |cx| {
            settle(cx);
            !has(format!("ask-{first_ask}-question-text"), cx)
                && has(format!("row-{first}-answered-0-answer"), cx)
        },
    );
    assert!(
        card_shows(&page, second, cx),
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
        !has(format!("ask-{second_ask}-question-text"), cx)
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    (id, shell, page, at, cx)
}

fn click(id: String, cx: &mut VisualTestContext) {
    to_question_of(&id, cx);
    cx.update(|window, cx| window.click(id, cx));
    cx.run_until_parked();
}

fn bounds(id: String, cx: &mut VisualTestContext) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    to_question_of(&id, cx);
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
    to_question_of(&id, cx);
    cx.update(|window, _| window.try_find(id).and_then(|found| found.selected()))
}

/// Answers the three questions: Yes with a note, a title, Alice and Carol.
fn answer_three(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) {
    let ask = seq_of(page, at, cx);
    click(format!("ask-{ask}-question-0-option-0"), cx);
    settle(cx);
    click(format!("ask-{ask}-note-0-add"), cx);
    settle(cx);
    note(page, at, 0, "Squash it first.", cx);
    write(page, at, 1, "Export orders as CSV", cx);
    click(format!("ask-{ask}-question-2-option-0"), cx);
    click(format!("ask-{ask}-question-2-option-2"), cx);
    settle(cx);
    click(format!("ask-{ask}-answer-send"), cx);
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
            let ask = seq_of(&page, *at, cx);
            card_shows(&page, *at, cx)
                && label(format!("ask-{ask}-question-title"), cx).as_deref()
                    == Some("Charts of the quarter")
        })
    });
    let at = questions(&page, cx)[0].0;
    let ask = seq_of(&page, at, cx);
    assert_eq!(
        label(format!("ask-{ask}-question-tag"), cx).as_deref(),
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
    let ask = seq_of(&page, at, cx);
    assert_eq!(
        questions(&page, cx)[0].1.lines().count(),
        5,
        "one ask, its text numbered"
    );
    for n in 0..3 {
        show(ask, n, cx);
        assert!(has(format!("ask-{ask}-question-{n}"), cx), "block {n}");
    }
    show(ask, 0, cx);
    assert!(has(format!("ask-{ask}-question-0-options"), cx));
    show(ask, 1, cx);
    assert!(
        !has(format!("ask-{ask}-question-1-options"), cx),
        "a question in words alone"
    );
    assert!(
        has(format!("ask-{ask}-answer-1-frame"), cx),
        "its field is there at once"
    );
    assert_eq!(
        label(format!("ask-{ask}-answer-1-field-label"), cx).as_deref(),
        Some("Your answer"),
        "a question in words alone keeps its labelled field"
    );
    show(ask, 0, cx);
    assert!(
        !has(format!("ask-{ask}-answer-0-frame"), cx),
        "the words of a question of options come with Something else"
    );
    show(ask, 2, cx);
    assert!(has(format!("ask-{ask}-question-2-option-2"), cx));

    // One choice: the second replaces the first. Several: both stay.
    click(format!("ask-{ask}-question-0-option-0"), cx);
    click(format!("ask-{ask}-question-0-option-1"), cx);
    click(format!("ask-{ask}-question-2-option-0"), cx);
    click(format!("ask-{ask}-question-2-option-1"), cx);
    settle(cx);
    assert_eq!(
        chosen(format!("ask-{ask}-question-0-option-0"), cx),
        Some(false)
    );
    assert_eq!(
        chosen(format!("ask-{ask}-question-0-option-1"), cx),
        Some(true)
    );
    assert_eq!(
        chosen(format!("ask-{ask}-question-2-option-0"), cx),
        Some(true)
    );
    assert_eq!(
        chosen(format!("ask-{ask}-question-2-option-1"), cx),
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
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    let row = format!("ask-{ask}-question-0-option-1");
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
        label(format!("ask-{ask}-question-0-other"), cx).as_deref(),
        Some("Something else…")
    );
    assert_eq!(
        label(format!("ask-{ask}-question-2-other"), cx).as_deref(),
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let ask = seq_of(&page, at, cx);
    let (card, line) = cx.update(|window, _| {
        (
            window.find(format!("ask-{ask}-question-text")).bounds(),
            window
                .find(format!("ask-{ask}-question-0-option-0-line"))
                .bounds(),
        )
    });
    assert!(line.right() <= card.right(), "{line:?} in {card:?}");
    assert!(line.size.height > px(36.), "on several lines: {line:?}");
}

/// The three questions answered in long words: a choice with a long unbroken note, a long
/// spaced answer, and the choices of the third left as they were.
fn answer_three_at_length(page: &Entity<SessionPage>, at: usize, cx: &mut VisualTestContext) {
    let ask = seq_of(page, at, cx);
    click(format!("ask-{ask}-question-0-option-0"), cx);
    settle(cx);
    click(format!("ask-{ask}-note-0-add"), cx);
    settle(cx);
    note(page, at, 0, guard::UNBROKEN, cx);
    write(page, at, 1, guard::SPACED, cx);
    click(format!("ask-{ask}-question-2-option-0"), cx);
    click(format!("ask-{ask}-question-2-option-2"), cx);
    settle(cx);
    click(format!("ask-{ask}-answer-send"), cx);
}

#[gpui_kit::test]
fn a_long_answer_stays_inside_its_answered_card(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    guard::narrow(cx);
    answer_three_at_length(&page, at, cx);
    until(&shell, cx, "the card folds to its answers", |cx| {
        settle(cx);
        !has(format!("ask-{ask}-question-text"), cx)
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

/// The "long ask" folded after an unbroken word in a "Something else" answer to its long
/// question: the headline row (check and words) and the question's ellipsis line at length.
fn long_ask_answered<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (usize, &'a mut VisualTestContext) {
    let id = session(user, "full", "long ask");
    let (shell, page, cx) = open_session(user, &id, cx);
    guard::narrow(cx);
    until(&shell, cx, "the card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let ask = seq_of(&page, at, cx);
    click(format!("ask-{ask}-question-0-other"), cx);
    settle(cx);
    write(&page, at, 0, guard::UNBROKEN, cx);
    click(format!("ask-{ask}-question-1-option-0"), cx);
    settle(cx);
    click(format!("ask-{ask}-answer-send"), cx);
    until(&shell, cx, "the card folds to its answers", |cx| {
        settle(cx);
        !has(format!("ask-{ask}-question-text"), cx)
            && has(format!("row-{at}-answered-1-answer"), cx)
    });
    settle(cx);
    (at, cx)
}

#[gpui_kit::test]
fn a_long_question_and_an_unbroken_answer_stay_inside_the_folded_card(cx: &mut TestAppContext) {
    let user = user();
    let (at, cx) = long_ask_answered(&user, cx);
    guard::assert_stays_inside(cx, format!("row-{at}-question"));
    // Both ends of the first question's rows are drawn and inside the card.
    let card = bounds(format!("row-{at}-question"), cx);
    for part in ["answered-0-question", "answered-0-headline"] {
        let part = bounds(format!("row-{at}-{part}"), cx);
        assert!(part.right() <= card.right(), "{part:?} in {card:?}");
    }
    let headline = bounds(format!("row-{at}-answered-0-headline"), cx);
    assert!(
        headline.size.height > px(36.),
        "the unbroken word wraps on lines: {headline:?}"
    );
    let question = bounds(format!("row-{at}-answered-0-question"), cx);
    assert!(question.size.height < px(24.), "one line: {question:?}");
    at_rest(cx);
}

#[gpui_kit::test]
fn the_whole_long_question_of_a_folded_card_shows_on_hover(cx: &mut TestAppContext) {
    let user = user();
    let (at, cx) = long_ask_answered(&user, cx);
    let line = format!("row-{at}-answered-0-question");
    assert!(!has(format!("{line}-tooltip"), cx), "no tooltip at rest");

    cx.update(|window, cx| window.hover(line.clone(), cx));
    settle(cx);
    assert_eq!(
        label(format!("{line}-tooltip"), cx).as_deref(),
        Some(format!("1 · {} {}", guard::UNBROKEN, guard::SPACED).as_str())
    );

    // The pointer leaves: it fades out and goes.
    cx.simulate_mouse_move(gpui_kit::point(px(5.), px(1790.)), None, Default::default());
    settle(cx);
    assert!(!has(format!("{line}-tooltip"), cx), "the tooltip left");
    at_rest(cx);
}

#[gpui_kit::test]
fn a_question_that_fits_has_no_tooltip_in_a_folded_card(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    click(format!("ask-{ask}-question-0-option-0"), cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("ask-{ask}-question-2-option-0"), cx);
    settle(cx);
    click(format!("ask-{ask}-answer-send"), cx);
    until(&shell, cx, "the card folds to its answers", |cx| {
        settle(cx);
        has(format!("row-{at}-answered-2-answer"), cx)
    });
    let line = format!("row-{at}-answered-0-question");
    cx.update(|window, cx| window.hover(line.clone(), cx));
    settle(cx);
    assert!(!has(format!("{line}-tooltip"), cx), "it fits: no tooltip");
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let ask = seq_of(&page, at, cx);
    // Docked (#777): each question in its turn stays inside the card, the card inside the dock,
    // and the line where it was asked inside its row.
    for n in 0..2 {
        show(ask, n, cx);
        guard::assert_stays_inside(cx, format!("ask-{ask}-question-text"));
        guard::assert_stays_inside(cx, "ask-dock");
    }
    guard::assert_stays_inside(cx, format!("row-{at}-question"));
}

#[gpui_kit::test]
fn several_choices_have_square_marks_and_say_several_once(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    assert!(has(format!("ask-{ask}-question-0-option-0-dot"), cx));
    assert!(!has(format!("ask-{ask}-question-0-option-0-tick"), cx));
    assert!(!has(format!("ask-{ask}-question-0-several"), cx));
    show(ask, 2, cx);
    assert!(has(format!("ask-{ask}-question-2-option-0-tick"), cx));
    assert!(!has(format!("ask-{ask}-question-2-option-0-dot"), cx));
    assert_eq!(
        label(format!("ask-{ask}-question-2-several"), cx).as_deref(),
        Some("several")
    );
    let said = cx.update(|window, _| window.find_all_by_label("several").len());
    assert_eq!(said, 1, "said once");
}

#[gpui_kit::test]
fn something_else_holds_a_named_field_and_needs_words(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    let card = card_of(&page, at, cx);
    click(format!("ask-{ask}-question-0-option-0"), cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("ask-{ask}-question-2-option-0"), cx);
    settle(cx);
    assert!(card.read_with(cx, |card, cx| card.is_ready(cx)));
    assert!(!has(format!("ask-{ask}-answer-0-frame"), cx), "folded");

    // Picked, it takes the place of the choice and holds its field, named.
    click(format!("ask-{ask}-question-0-other"), cx);
    settle(cx);
    assert_eq!(
        chosen(format!("ask-{ask}-question-0-option-0"), cx),
        Some(false)
    );
    assert_eq!(
        chosen(format!("ask-{ask}-question-0-other"), cx),
        Some(true)
    );
    assert!(has(format!("ask-{ask}-answer-0-frame"), cx));
    assert_eq!(
        label(format!("ask-{ask}-answer-0-frame"), cx).as_deref(),
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
    click(format!("ask-{ask}-question-0-other-label"), cx);
    settle(cx);
    assert!(
        !has(format!("ask-{ask}-answer-0-frame"), cx),
        "folded again"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn the_field_of_something_else_is_in_its_row_right_of_the_mark_and_focused(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    let other = format!("ask-{ask}-question-0-other");
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
        bounds(format!("ask-{ask}-answer-0-frame"), cx),
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
    let ask = seq_of(&page, at, cx);
    let card = card_of(&page, at, cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("ask-{ask}-question-2-option-0"), cx);
    click(format!("ask-{ask}-question-0-other"), cx);
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
        !has(format!("ask-{ask}-answer-0-frame"), cx),
        "the field gave way once sent from the field itself"
    );
    assert!(!field_focused(&page, at, 0, cx), "and holds no focus");
}

#[gpui_kit::test]
fn the_field_in_the_row_grows_to_several_lines_inside_it(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    let (other, frame) = (
        format!("ask-{ask}-question-0-other"),
        format!("ask-{ask}-answer-0-frame"),
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
    let ask = seq_of(&page, at, cx);
    let other = format!("ask-{ask}-question-2-other");
    click(format!("ask-{ask}-question-2-option-0"), cx);
    click(other.clone(), cx);
    settle(cx);
    assert!(has(format!("{other}-tick"), cx), "a square mark");
    assert_eq!(chosen(other.clone(), cx), Some(true));
    assert_eq!(
        chosen(format!("ask-{ask}-question-2-option-0"), cx),
        Some(true),
        "the other choice stays"
    );
    let (row, mark, field) = (
        bounds(other.clone(), cx),
        bounds(format!("{other}-mark"), cx),
        bounds(format!("ask-{ask}-answer-2-frame"), cx),
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
    let ask = seq_of(&page, at, cx);
    let card = card_of(&page, at, cx);
    click(format!("ask-{ask}-question-0-other"), cx);
    write(&page, at, 0, "Both, in that order", cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("ask-{ask}-question-2-option-0"), cx);
    show(ask, 0, cx);
    settle(cx);
    assert!(has(format!("ask-{ask}-answer-0-frame"), cx));
    click(format!("ask-{ask}-answer-send"), cx);
    until(&shell, cx, "the answer went", |cx| {
        card.read_with(cx, |card, cx| card.is_sent(cx))
    });
    settle(cx);
    assert!(
        !has(format!("ask-{ask}-answer-0-frame"), cx),
        "the field of the row folded away, no longer editable"
    );
    assert!(!has(format!("ask-{ask}-answer-1-frame"), cx));
    assert!(
        !field_focused(&page, at, 0, cx),
        "nothing is typed into a field that gave way"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn nothing_under_the_options_holds_the_words_of_something_else(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    click(format!("ask-{ask}-question-0-other"), cx);
    click(format!("ask-{ask}-question-2-other"), cx);
    settle(cx);
    assert!(has(format!("ask-{ask}-answer-2-frame"), cx));
    show(ask, 0, cx);
    assert!(has(format!("ask-{ask}-answer-0-frame"), cx));
    for n in [0, 2] {
        show(ask, n, cx);
        assert!(
            !has(format!("ask-{ask}-answer-{n}-field"), cx),
            "no separate field under the options of question {n}"
        );
        assert!(!has(format!("ask-{ask}-answer-{n}-field-label"), cx));
        assert!(!has(format!("ask-{ask}-question-{n}-words"), cx));
    }
}

#[gpui_kit::test]
fn letting_go_of_something_else_folds_its_field_and_keeps_the_words(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    let other = format!("ask-{ask}-question-0-other");
    let alone = bounds(other.clone(), cx).size.height;
    click(other.clone(), cx);
    settle(cx);
    write(&page, at, 0, "Both, in that order", cx);

    click(format!("{other}-label"), cx);
    settle(cx);
    assert!(!has(format!("ask-{ask}-answer-0-frame"), cx), "folded away");
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
    assert!(has(format!("ask-{ask}-answer-0-frame"), cx));
    let words = card_of(&page, at, cx).read_with(cx, |card, cx| card.field(0).read(cx).value());
    assert_eq!(words.as_ref(), "Both, in that order", "kept as it was");
    at_rest(cx);
}

#[gpui_kit::test]
fn add_a_note_is_offered_on_a_question_of_options_and_sends_the_note(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    assert_eq!(
        label(format!("ask-{ask}-note-0-add"), cx).as_deref(),
        Some("+ Add a note"),
        "before a choice too"
    );
    show(ask, 1, cx);
    assert!(
        !has(format!("ask-{ask}-note-1-add"), cx),
        "a question without options is answered in words"
    );
    assert_eq!(
        label(format!("ask-{ask}-note-2-add"), cx).as_deref(),
        Some("+ Add a note")
    );
    click(format!("ask-{ask}-question-0-option-0"), cx);
    settle(cx);
    assert_eq!(
        label(format!("ask-{ask}-note-0-add"), cx).as_deref(),
        Some("+ Add a note")
    );
    assert!(!has(format!("ask-{ask}-note-0-frame"), cx));
    click(format!("ask-{ask}-note-0-add"), cx);
    settle(cx);
    assert!(has(format!("ask-{ask}-note-0-frame"), cx), "its field");
    assert_eq!(
        label(format!("ask-{ask}-note-0-field-label"), cx).as_deref(),
        Some("Note with your choice")
    );
    assert!(!has(format!("ask-{ask}-note-0-add"), cx), "the button left");
    note(&page, at, 0, "Squash it first.", cx);
    let answers = card_of(&page, at, cx).read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[0].choices, ["Yes"]);
    assert_eq!(answers[0].text, "Squash it first.");

    // The note goes with the choice.
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("ask-{ask}-question-2-option-1"), cx);
    settle(cx);
    click(format!("ask-{ask}-answer-send"), cx);
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
    let ask = seq_of(&page, at, cx);
    let card = card_of(&page, at, cx);
    let why = format!("ask-{ask}-answer-why");
    assert_eq!(
        label(why.clone(), cx).as_deref(),
        Some("Answer each question to send.")
    );
    click(format!("ask-{ask}-answer-send"), cx);
    settle(cx);
    assert!(
        !card.read_with(cx, |card, cx| card.is_sent(cx)),
        "Answer waits"
    );
    click(format!("ask-{ask}-question-0-option-0"), cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("ask-{ask}-question-2-option-1"), cx);
    settle(cx);
    assert_eq!(
        label(why, cx).as_deref(),
        Some("Sent to the agent as one message.")
    );
    assert_eq!(
        label(format!("ask-{ask}-answer-keys"), cx).as_deref(),
        Some("Ctrl+Enter")
    );
    click(format!("ask-{ask}-answer-send"), cx);
    until(&shell, cx, "the answer went", |cx| {
        card.read_with(cx, |card, cx| card.is_sent(cx))
    });
}

#[gpui_kit::test]
fn the_count_says_how_many_are_answered(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    let count = format!("ask-{ask}-question-count");
    assert_eq!(label(count.clone(), cx).as_deref(), Some("0 of 3 answered"));
    click(format!("ask-{ask}-question-0-option-0"), cx);
    settle(cx);
    assert_eq!(label(count.clone(), cx).as_deref(), Some("1 of 3 answered"));
    write(&page, at, 1, "Export orders as CSV", cx);
    settle(cx);
    assert_eq!(label(count.clone(), cx).as_deref(), Some("2 of 3 answered"));
    // Something else, with no words yet, is not an answer.
    click(format!("ask-{ask}-question-2-other"), cx);
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
    let ask = seq_of(&page, at, cx);
    let card = card_of(&page, at, cx);
    click(format!("ask-{ask}-question-0-option-0"), cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    assert!(
        !card.read_with(cx, |card, cx| card.is_ready(cx)),
        "the third has no answer"
    );
    click(format!("ask-{ask}-answer-send"), cx);
    settle(cx);
    assert!(
        !card.read_with(cx, |card, cx| card.is_sent(cx)),
        "nothing sent"
    );
    // Words alone answer a question of options too, through Something else.
    click(format!("ask-{ask}-question-2-other"), cx);
    write(&page, at, 2, "Whoever is free", cx);
    assert!(card.read_with(cx, |card, cx| card.is_ready(cx)));
    click(format!("ask-{ask}-answer-send"), cx);
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let ask = seq_of(&page, at, cx);
    assert!(has(format!("ask-{ask}-question-0"), cx));
    assert!(!has(format!("ask-{ask}-question-1"), cx), "one block");
    assert!(
        !has(format!("ask-{ask}-question-0-options"), cx),
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
    let ask = seq_of(&page, at, cx);
    answer_three(&page, at, cx);
    until(&shell, cx, "the card folds to its answers", |cx| {
        settle(cx);
        !has(format!("ask-{ask}-question-text"), cx)
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
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
                .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let page = session_page(&shell, cx);
    let ask = seq_of(&page, questions(&page, cx)[0].0, cx);
    assert_eq!(
        label(format!("ask-{ask}-answer-waits"), cx).as_deref(),
        Some(ANSWER_WAITS)
    );
    assert!(
        !has(format!("ask-{ask}-answer-0-frame"), cx),
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
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
                .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let page = session_page(&shell, cx);
    let ask = seq_of(&page, questions(&page, cx)[0].0, cx);
    assert_eq!(
        label(format!("ask-{ask}-answer-waits"), cx).as_deref(),
        Some(ANSWER_WAITS)
    );
    assert!(
        !has(format!("ask-{ask}-answer-0-frame"), cx),
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
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let ask = seq_of(&page, at, cx);
    let card = card_of(&page, at, cx);

    // No field is open: the card has only rows, and a click on one is all the user did.
    click(format!("ask-{ask}-question-0-option-0"), cx);
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
    let ask = seq_of(&page, at, cx);
    let card = card_of(&page, at, cx);
    click(format!("ask-{ask}-question-0-option-0"), cx);
    write(&page, at, 1, "Export orders as CSV", cx);
    click(format!("ask-{ask}-question-2-option-0"), cx);
    settle(cx);
    assert!(card.read_with(cx, |card, cx| card.is_ready(cx)));

    // A question of several choices keeps Alice, and "Something else…" is picked beside her.
    click(format!("ask-{ask}-question-2-other"), cx);
    settle(cx);
    assert_eq!(
        chosen(format!("ask-{ask}-question-2-option-0"), cx),
        Some(true)
    );
    assert_eq!(
        chosen(format!("ask-{ask}-question-2-other"), cx),
        Some(true)
    );
    assert!(
        !card.read_with(cx, |card, cx| card.is_ready(cx)),
        "it needs its words"
    );
    assert_eq!(card.read_with(cx, |card, cx| card.answered_count(cx)), 2);
    assert_eq!(
        label(format!("ask-{ask}-question-count"), cx).as_deref(),
        Some("2 of 3 answered")
    );
    click(format!("ask-{ask}-answer-send"), cx);
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
fn letting_the_choice_go_keeps_the_note(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = three_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    click(format!("ask-{ask}-question-0-option-0"), cx);
    settle(cx);
    click(format!("ask-{ask}-note-0-add"), cx);
    settle(cx);
    note(&page, at, 0, "Squash it first.", cx);
    assert!(has(format!("ask-{ask}-note-0-frame"), cx));

    // The choice goes: a note may come before a choice, so it stays with its words.
    click(format!("ask-{ask}-question-0-option-0"), cx);
    settle(cx);
    assert!(has(format!("ask-{ask}-note-0-frame"), cx), "still open");
    let card = card_of(&page, at, cx);
    let kept = card.read_with(cx, |card, cx| card.note(0).read(cx).value().to_string());
    assert_eq!(kept, "Squash it first.");
    assert_eq!(
        card.read_with(cx, |card, cx| card.answered_count(cx)),
        0,
        "a note alone is no answer"
    );

    // Choosing again sends the note with the new choice.
    click(format!("ask-{ask}-question-0-option-1"), cx);
    settle(cx);
    let answers = card.read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[0].choices, ["Not yet"]);
    assert_eq!(answers[0].text, "Squash it first.");
}

#[gpui_kit::test]
fn a_note_written_before_the_choice_comes_back_with_previous(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = four_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    cx.simulate_resize(size(px(1440.), px(1800.)));
    settle(cx);
    cx.update(|window, cx| window.click(format!("ask-{ask}-note-0-add"), cx));
    settle(cx);
    assert!(has(format!("ask-{ask}-note-0-frame"), cx), "a note first");
    note(&page, at, 0, "The one we run already.", cx);

    // The choice made, the next question comes.
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-0-option-0"), cx));
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("2 of 4"));

    // Previous: the question again, with its choice and its note.
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-previous"), cx));
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("1 of 4"));
    assert_eq!(
        chosen(format!("ask-{ask}-question-0-option-0"), cx),
        Some(true)
    );
    assert!(has(format!("ask-{ask}-note-0-frame"), cx), "its note");
    let answers = card_of(&page, at, cx).read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[0].choices, ["Postgres"]);
    assert_eq!(answers[0].text, "The one we run already.");
    at_rest(cx);
}

/// A session that asked the four questions and wrote after them, its card docked: the row of
/// its line.
fn four_asked<'a>(
    user: &User,
    cx: &'a mut TestAppContext,
) -> (
    String,
    Entity<Shell>,
    Entity<SessionPage>,
    usize,
    &'a mut VisualTestContext,
) {
    let id = session(user, "full", "ask four");
    let (shell, page, cx) = open_session(user, &id, cx);
    until(&shell, cx, "the docked card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    (id, shell, page, at, cx)
}

fn following(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> bool {
    page.read_with(cx, |page, cx| page.conversation().read(cx).is_following())
}

fn step(ask: u64, cx: &mut VisualTestContext) -> Option<String> {
    label(format!("ask-{ask}-question-step"), cx)
}

#[gpui_kit::test]
fn a_waiting_ask_is_docked_above_the_composer_and_stays_in_view_while_the_conversation_scrolls(
    cx: &mut TestAppContext,
) {
    let user = user();
    let (id, _, page, at, cx) = four_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    let (frame, card, composer, conversation) = (
        bounds("ask-dock-frame".into(), cx),
        bounds(format!("ask-{ask}-question-text"), cx),
        bounds("reply".into(), cx),
        bounds(format!("conversation-{id}"), cx),
    );
    assert!(
        frame.contains(&card.origin),
        "the card is in the dock: {card:?} {frame:?}"
    );
    assert!(
        conversation.bottom() <= frame.top() && frame.bottom() <= composer.top(),
        "under the conversation, above the composer: {conversation:?} {frame:?} {composer:?}"
    );
    assert_eq!(
        label(format!("row-{at}-question-line"), cx).as_deref(),
        Some("Asked you 4 questions · answer below"),
        "one line where it was asked"
    );
    assert!(docked(&page, cx).is_some_and(|card| card.read_with(cx, |card, _| card.seq() == ask)));

    // Read from the top of the conversation, longer than its height: the card has not moved.
    assert!(following(&page, cx), "at the end");
    let middle = conversation.center();
    for _ in 0..4 {
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: middle,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(400.))),
            ..Default::default()
        });
        settle(cx);
    }
    assert!(!following(&page, cx), "the conversation scrolled up");
    assert_eq!(bounds(format!("ask-{ask}-question-text"), cx), card);
    at_rest(cx);
}

#[gpui_kit::test]
fn the_dock_is_narrower_than_the_composer(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = four_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    let (card, composer) = (
        bounds(format!("ask-{ask}-question-text"), cx),
        bounds("reply".into(), cx),
    );
    let (left, right) = (
        card.left() - composer.left(),
        composer.right() - card.right(),
    );
    assert!(
        left >= px(12.) && right >= px(12.),
        "inset: {card:?} {composer:?}"
    );
    assert_eq!(left, right, "on both sides alike");
}

#[gpui_kit::test]
fn one_question_at_a_time_with_previous_and_next(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = four_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    assert_eq!(step(ask, cx).as_deref(), Some("1 of 4"));
    assert!(has(format!("ask-{ask}-question-0"), cx));
    for n in 1..4 {
        assert!(!has(format!("ask-{ask}-question-{n}"), cx), "one at a time");
    }
    let dot = |n: usize| format!("ask-{ask}-question-dots-{n}");
    assert!(chosen(dot(0), cx) == Some(true) && chosen(dot(1), cx) == Some(false));

    // Next: the second slides in from the right.
    let rest = bounds(format!("ask-{ask}-question-0"), cx).left();
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-next"), cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.executor()
        .advance_clock(gpui_design_system::motion::PANEL / 3);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    let moving = bounds(format!("ask-{ask}-question-1"), cx).left();
    assert!(moving > rest, "it slides in: {moving:?} {rest:?}");
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("2 of 4"));
    assert_eq!(bounds(format!("ask-{ask}-question-1"), cx).left(), rest);
    assert!(!has(format!("ask-{ask}-question-0"), cx));
    assert_eq!(chosen(dot(1), cx), Some(true));

    // The arrows, from the card itself.
    let card = card_of(&page, at, cx);
    cx.update(|window, cx| window.focus(&card.read(cx).focus_handle(cx), cx));
    cx.simulate_keystrokes("right");
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("3 of 4"));
    cx.simulate_keystrokes("left");
    cx.simulate_keystrokes("left");
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("1 of 4"));

    // In a field, they move the caret.
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-next"), cx));
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-next"), cx));
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-next"), cx));
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("4 of 4"));
    let field = card.read_with(cx, |card, _| card.field(3).clone());
    cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
    cx.simulate_input("ok");
    cx.simulate_keystrokes("left");
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("4 of 4"));
    at_rest(cx);
}

#[gpui_kit::test]
fn picking_an_option_moves_to_the_next_unanswered_question(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = four_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-0-option-0"), cx));
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("2 of 4"));
    let dot = format!("ask-{ask}-question-dots-0");
    assert_eq!(label(dot, cx).as_deref(), Some("1 of 4, done"));

    cx.update(|window, cx| window.click(format!("ask-{ask}-question-1-option-1"), cx));
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("3 of 4"));

    // Several choices: it stays for more, and moves on with Next.
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-2-option-0"), cx));
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("3 of 4"));

    // Changing an answer goes on to the one still waiting, past those answered.
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-previous"), cx));
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("2 of 4"));
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-1-option-0"), cx));
    settle(cx);
    assert_eq!(step(ask, cx).as_deref(), Some("4 of 4"));
    at_rest(cx);
}

#[gpui_kit::test]
fn the_line_in_the_conversation_brings_the_focus_to_the_dock(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = four_asked(&user, cx);
    let card = card_of(&page, at, cx);
    let composer = page.read_with(cx, |page, cx| {
        page.reply().unwrap().read(cx).field().clone()
    });
    cx.update(|window, cx| window.focus(&composer.read(cx).focus_handle(cx), cx));
    assert!(!cx.update(|window, cx| card.read(cx).has_focus(window)));

    cx.update(|window, cx| window.click(format!("row-{at}-question-line"), cx));
    cx.run_until_parked();
    assert!(
        cx.update(|window, cx| card.read(cx).has_focus(window)),
        "the dock has the focus"
    );
    assert!(card.read_with(cx, |card, _| card.pulsing()), "it lights");
    settle(cx);
    cx.executor()
        .advance_clock(gpui_design_system::motion::PANEL * 2);
    settle(cx);
    assert!(!card.read_with(cx, |card, _| card.pulsing()), "once");
    at_rest(cx);
}

#[gpui_kit::test]
fn once_answered_the_dock_folds_and_the_conversation_shows_the_answers(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "draw the charts");
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "the docked card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    let at = questions(&page, cx)[0].0;
    assert_eq!(
        label(format!("row-{at}-question-line"), cx).as_deref(),
        Some("Asked you a question · answer below")
    );
    answer(&page, at, "Drop it.", cx);
    settle(cx);
    let open = bounds("ask-dock-open".into(), cx).size.height;
    release(&user);
    // The clock holds still meanwhile: the fold starts at the frame the agent took the answer.
    until(&shell, cx, "the agent took the answer", |cx| {
        docked(&page, cx).is_none()
    });
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.executor()
        .advance_clock(gpui_design_system::motion::PANEL / 3);
    cx.update(|window, cx| window.simulate_next_frame(cx));
    let folding = bounds("ask-dock-open".into(), cx).size.height;
    assert!(
        px(0.) < folding && folding < open,
        "it folds away: {folding:?} of {open:?}"
    );
    until(
        &shell,
        cx,
        "the dock folds, the answers in the conversation",
        |cx| {
            settle(cx);
            !has("ask-dock", cx)
                && !has(format!("row-{at}-question-line"), cx)
                && label(format!("row-{at}-answered-0-answer"), cx).as_deref() == Some("Drop it.")
        },
    );
    let (conversation, answered) = (
        bounds(format!("conversation-{id}"), cx),
        bounds(format!("row-{at}-answered-0-answer"), cx),
    );
    assert!(
        conversation.contains(&answered.origin),
        "where it was asked"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn the_conversation_keeps_half_the_height_and_the_dock_scrolls_past_it(cx: &mut TestAppContext) {
    let user = user();
    let (id, _, page, at, cx) = four_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    cx.simulate_resize(size(px(1440.), px(520.)));
    settle(cx);
    let (page, conversation, frame, card) = (
        bounds("session-column".into(), cx),
        bounds(format!("conversation-{id}"), cx),
        bounds("ask-dock-frame".into(), cx),
        bounds(format!("ask-{ask}-question-text"), cx),
    );
    assert!(
        conversation.size.height >= page.size.height / 2.,
        "half the page: {conversation:?} {page:?}"
    );
    assert!(
        frame.size.height < card.size.height,
        "the dock scrolls: {frame:?} {card:?}"
    );
    at_rest(cx);
}

#[gpui_kit::test]
fn two_asks_waiting_show_the_oldest_first(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "ask twice");
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "two asks, the oldest docked", |cx| {
        let open = questions(&page, cx);
        open.len() == 2 && card_shows(&page, open[0].0, cx)
    });
    settle(cx);
    let (first, second) = (questions(&page, cx)[0].0, questions(&page, cx)[1].0);
    assert!(!card_shows(&page, second, cx), "one at a time");
    assert_eq!(label("ask-dock-count", cx).as_deref(), Some("1 of 2 asks"));
    assert!(has(format!("row-{second}-question-line"), cx));

    answer(&page, first, "Drop it.", cx);
    release(&user);
    until(&shell, cx, "the second comes", |cx| {
        settle(cx);
        card_shows(&page, second, cx) && !card_shows(&page, first, cx)
    });
    settle(cx);
    assert!(!has("ask-dock-count", cx), "one left");
    at_rest(cx);
}

/// Whether the field of the composer of `page` has the focus.
fn composer_focused(page: &Entity<SessionPage>, cx: &mut VisualTestContext) -> bool {
    let field = page.read_with(cx, |page, cx| {
        page.reply().unwrap().read(cx).field().clone()
    });
    cx.update(|window, cx| field.read(cx).focus_handle(cx).is_focused(window))
}

fn focus_composer(page: &Entity<SessionPage>, cx: &mut VisualTestContext) {
    let field = page.read_with(cx, |page, cx| {
        page.reply().unwrap().read(cx).field().clone()
    });
    cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
    cx.run_until_parked();
}

/// Whether the focus is in `card`, on the card itself or in one of its fields.
fn in_card(card: &Entity<QuestionCard>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| card.read(cx).focus_handle(cx).contains_focused(window, cx))
}

#[gpui_kit::test]
fn a_draft_answer_stays_while_a_sub_agent_s_conversation_is_shown(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "ask beside a helper");
    let (shell, page, cx) = open_session(&user, &id, cx);
    // Tall enough for the whole card: a click needs its target in sight.
    cx.simulate_resize(size(px(1440.), px(1800.)));
    until(
        &shell,
        cx,
        "the docked card, and the helper in the tree",
        |cx| {
            questions(&page, cx)
                .first()
                .is_some_and(|(at, _)| card_shows(&page, *at, cx))
                && page.read_with(cx, |page, _| page.tree().len() == 2)
        },
    );
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let ask = seq_of(&page, at, cx);
    let card = card_of(&page, at, cx);
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-0-option-1"), cx));
    settle(cx);
    cx.update(|window, cx| window.click(format!("ask-{ask}-note-1-add"), cx));
    settle(cx);
    note(&page, at, 1, "Not yet.", cx);
    let helper = page.read_with(cx, |page, _| {
        page.tree()
            .iter()
            .find(|agent| agent.parent.is_some())
            .unwrap()
            .id
            .clone()
    });

    // The helper's conversation: the ask is the root's, its card stays as the user left it.
    page.update(cx, |page, cx| page.show(&helper, cx));
    until(&shell, cx, "the helper's conversation", |cx| {
        page.read_with(cx, |page, cx| {
            page.shown() == helper && !page.conversation().read(cx).rows().is_empty()
        })
    });
    settle(cx);
    assert!(
        has(format!("ask-{ask}-question-text"), cx),
        "the dock still shows it"
    );
    assert!(
        page.read_with(cx, |page, cx| page.ask_dock().read(cx).card(ask, cx)
            == Some(&card)),
        "the same card"
    );

    // Back to the root's: what was chosen and written is there.
    page.update(cx, |page, cx| page.show(&id, cx));
    until(&shell, cx, "the root's conversation", |cx| {
        page.read_with(cx, |page, _| page.shown() == id)
            && !questions(&page, cx).is_empty()
            && has(format!("ask-{ask}-question-text"), cx)
    });
    settle(cx);
    assert_eq!(card_of(&page, at, cx), card, "the same card");
    let answers = card.read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[0].choices, ["SQLite"]);
    assert_eq!(answers[1].text, "Not yet.");
    assert_eq!(step(ask, cx).as_deref(), Some("2 of 2"));
}

#[gpui_kit::test]
fn a_draft_answer_stays_when_the_session_is_left_and_opened_again(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "ask four");
    let other = user
        .call("agent.run", json!({"project": "app", "message": "hello"}))
        .unwrap()["agent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "the docked card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    let ask = seq_of(&page, at, cx);
    cx.update(|window, cx| window.click(format!("ask-{ask}-question-0-option-1"), cx));
    settle(cx);
    write(&page, at, 3, "Keep it small.", cx);
    assert_eq!(step(ask, cx).as_deref(), Some("2 of 4"));

    // Another session, then this one again: a new page, the same draft.
    cx.update(|window, cx| window.click(session_id(&other), cx));
    until(&shell, cx, "the other page", |cx| {
        session_page(&shell, cx).read_with(cx, |p, _| p.id() == other)
    });
    settle(cx);
    cx.update(|window, cx| window.click(session_id(&id), cx));
    until(&shell, cx, "this page again, its card", |cx| {
        let page = session_page(&shell, cx);
        page.read_with(cx, |p, _| p.id() == id)
            && !questions(&page, cx).is_empty()
            && has(format!("ask-{ask}-question-text"), cx)
    });
    settle(cx);
    let page = session_page(&shell, cx);
    let at = questions(&page, cx)[0].0;
    let answers = card_of(&page, at, cx).read_with(cx, |card, cx| card.answers(cx));
    assert_eq!(answers[0].choices, ["SQLite"]);
    assert_eq!(answers[3].text, "Keep it small.");
    assert_eq!(
        step(ask, cx).as_deref(),
        Some("2 of 4"),
        "where it was left"
    );
    assert!(
        in_card(&card_of(&page, at, cx), cx),
        "the page opened with an ask waiting: the dock has the focus"
    );
}

#[gpui_kit::test]
fn the_steps_stay_in_sight_while_the_dock_scrolls(cx: &mut TestAppContext) {
    let user = user();
    let (_, _, page, at, cx) = four_asked(&user, cx);
    let ask = seq_of(&page, at, cx);
    cx.simulate_resize(size(px(1440.), px(600.)));
    settle(cx);
    let frame = bounds("ask-dock-frame".into(), cx);
    let answer = bounds(format!("ask-{ask}-answer-send"), cx);
    assert!(answer.bottom() > frame.bottom(), "Answer is out of sight");

    // Down to Answer, as the user scrolls the dock.
    let over = bounds("ask-dock-scroll".into(), cx).center();
    for _ in 0..4 {
        cx.simulate_event(gpui_kit::ScrollWheelEvent {
            position: over,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-400.))),
            ..Default::default()
        });
        settle(cx);
    }
    let frame = bounds("ask-dock-frame".into(), cx);
    let answer = bounds(format!("ask-{ask}-answer-send"), cx);
    assert!(
        answer.top() >= frame.top() && answer.bottom() <= frame.bottom(),
        "scrolled to Answer: {answer:?} in {frame:?}"
    );
    for part in ["question-previous", "question-step", "question-next"] {
        let steps = bounds(format!("ask-{ask}-{part}"), cx);
        assert!(
            steps.top() >= frame.top() && steps.bottom() <= frame.bottom(),
            "{part} stays in sight: {steps:?} in {frame:?}"
        );
    }
    at_rest(cx);
}

#[gpui_kit::test]
fn the_dock_takes_the_focus_once_when_the_page_opens_with_an_ask_waiting(cx: &mut TestAppContext) {
    let user = user();
    let (_, shell, page, at, cx) = four_asked(&user, cx);
    let card = card_of(&page, at, cx);
    assert!(in_card(&card, cx), "the page opened on an ask waiting");

    // The user goes to the composer: the dock leaves the focus there.
    focus_composer(&page, cx);
    cx.simulate_input("Mean");
    for _ in 0..5 {
        until(&shell, cx, "a frame", |_| true);
        settle(cx);
    }
    assert!(composer_focused(&page, cx), "once");
    assert!(!in_card(&card, cx));
}

#[gpui_kit::test]
fn an_ask_asked_later_never_takes_the_focus_from_the_composer(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "Export orders");
    eventually("its turn ended", || state_of(&user, &id) == "idle");
    let (shell, page, cx) = open_session(&user, &id, cx);
    settle(cx);
    focus_composer(&page, cx);
    cx.simulate_input("Mean");

    user.call("agent.send", json!({"agent": id, "message": "pick one"}))
        .unwrap();
    until(&shell, cx, "the docked card", |cx| {
        questions(&page, cx)
            .first()
            .is_some_and(|(at, _)| card_shows(&page, *at, cx))
    });
    settle(cx);
    let at = questions(&page, cx)[0].0;
    assert!(composer_focused(&page, cx), "the user types on");
    assert!(!in_card(&card_of(&page, at, cx), cx));
}

#[gpui_kit::test]
fn once_an_ask_is_answered_the_focus_goes_to_the_next_card(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "ask twice");
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "two asks, the oldest docked", |cx| {
        let open = questions(&page, cx);
        open.len() == 2 && card_shows(&page, open[0].0, cx)
    });
    settle(cx);
    let (first, second) = (questions(&page, cx)[0].0, questions(&page, cx)[1].0);
    let card = card_of(&page, first, cx);
    assert!(in_card(&card, cx), "the oldest has the focus");

    // Answered from its card, which keeps the focus until the agent takes it.
    write(&page, first, 0, "Drop it.", cx);
    cx.update(|window, cx| window.focus(&card.read(cx).focus_handle(cx), cx));
    cx.simulate_keystrokes("ctrl-enter");
    release(&user);
    until(&shell, cx, "the second comes", |cx| {
        settle(cx);
        card_shows(&page, second, cx) && !card_shows(&page, first, cx)
    });
    settle(cx);
    assert!(
        in_card(&card_of(&page, second, cx), cx),
        "the focus went to the next card"
    );
    at_rest(cx);
}
