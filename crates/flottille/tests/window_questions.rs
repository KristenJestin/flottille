//! The scenarios of #519 in the window: a question a root asks the user with `ask_user` in the
//! middle of its turn shows at once, as a card at its place in the conversation with the field
//! that answers it, in the bell and as "needs you" on the session's row, until its answer is
//! taken. On GPUI's test platform in front of a real daemon whose agents are the fake ACP agent,
//! with a throwaway user. No window opens on a screen.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::User;
use flottille_ui::AgentState;
use flottille_ui::omarchy::Environment;
use flottille_window::Shell;
use flottille_window::conversation::Block;
use flottille_window::link::Connect;
use flottille_window::session::{ANSWER_WAITS, ANSWERS_ALL, SessionPage};
use flottille_window::tree::session_id;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, VisualTestContext, px, size};
use serde_json::{Value, json};

const QUESTION: &str =
    "**POC charts**: blocked by the minimum of 3 points?\n\n- keep the minimum\n- drop it";
const SECOND: &str = "Which colour for the axis?";

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
        {"when": "Drop it", "do": [{"say": "Dropped."}]}
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
    cx.update(|cx| flottille_ui::init_with(env, cx));
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

/// Whether the card of the question of row `at` shows, its text and its field.
fn card_shows(at: usize, cx: &mut VisualTestContext) -> bool {
    has(format!("row-{at}-question-text"), cx) && has(format!("row-{at}-answer-frame"), cx)
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
    let answer = page
        .read_with(cx, |page, cx| {
            page.conversation().read(cx).rows()[at]
                .read(cx)
                .answer()
                .cloned()
        })
        .expect("the card's field");

    // Answered while the agent works: it waits for the end of the turn, and says so.
    let field = answer.read_with(cx, |a, _| a.field().clone());
    field.update_in(cx, |field, window, cx| {
        field.set_value("Drop it.", window, cx)
    });
    answer.update(cx, |answer, cx| answer.send(cx));
    until(&shell, cx, "the answer went", |cx| {
        answer.read_with(cx, |a, cx| a.is_sent(cx))
    });
    settle(cx);
    assert_eq!(
        label(format!("row-{at}-answer-waits"), cx).as_deref(),
        Some(ANSWER_WAITS)
    );
    assert!(has(format!("row-{at}-question-text"), cx), "still open");

    // The turn ends, the agent takes the answer: the card folds away, the row and the bell
    // no longer say it.
    release(&user);
    until(&shell, cx, "the answer is taken", |cx| {
        blocks(&page, cx)
            .iter()
            .any(|b| matches!(b, Block::Text { text } if text == "Dropped."))
    });
    until(&shell, cx, "the card folds away", |cx| {
        settle(cx);
        !has(format!("row-{at}-question-text"), cx)
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

/// The card's field of the question of row `at`.
fn answer_of(
    page: &Entity<SessionPage>,
    at: usize,
    cx: &mut VisualTestContext,
) -> Entity<flottille_window::session::AnswerBox> {
    page.read_with(cx, |page, cx| {
        page.conversation().read(cx).rows()[at]
            .read(cx)
            .answer()
            .cloned()
    })
    .expect("the card's field")
}

fn answer(
    page: &Entity<SessionPage>,
    at: usize,
    text: &str,
    cx: &mut VisualTestContext,
) -> Entity<flottille_window::session::AnswerBox> {
    let answer = answer_of(page, at, cx);
    let field = answer.read_with(cx, |a, _| a.field().clone());
    field.update_in(cx, |field, window, cx| field.set_value(text, window, cx));
    answer.update(cx, |answer, cx| answer.send(cx));
    answer
}

#[gpui_kit::test]
fn answering_one_of_two_questions_answers_both_and_says_so(cx: &mut TestAppContext) {
    let user = user();
    let id = session(&user, "full", "ask twice");
    let (shell, page, cx) = open_session(&user, &id, cx);
    until(&shell, cx, "both cards", |cx| {
        let open = questions(&page, cx);
        open.len() == 2 && open.iter().all(|(at, _)| card_shows(*at, cx))
    });
    settle(cx);
    let (first, second) = (questions(&page, cx)[0].0, questions(&page, cx)[1].0);
    for at in [first, second] {
        assert_eq!(
            label(format!("row-{at}-answer-all"), cx).as_deref(),
            Some(ANSWERS_ALL),
            "each card says where its answer goes"
        );
    }

    answer(&page, first, "Keep it, and blue.", cx);
    release(&user);
    until(&shell, cx, "both cards fold away", |cx| {
        settle(cx);
        !has(format!("row-{first}-question-text"), cx)
            && !has(format!("row-{second}-question-text"), cx)
    });
    let prompt = user
        .received_requests("session/prompt")
        .iter()
        .map(|r| r["params"]["prompt"].to_string())
        .find(|p| p.contains("Keep it, and blue."))
        .expect("the answer reached the agent");
    assert!(
        prompt.contains("POC charts"),
        "it quotes the first: {prompt}"
    );
    assert!(prompt.contains(SECOND), "it quotes the second: {prompt}");
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
        !has(format!("row-{at}-answer-frame"), cx),
        "no field to answer again"
    );
    release(&user);
}
