//! The card of an ask of the root to the user (`docs/tools.md`, "ask"): who asks, by the
//! session's title, then one block per question, each with its options as chips (one or several
//! to choose) and a field of several lines for the user's own words (`Enter` breaks a line,
//! `Ctrl+Enter` sends). One Answer for the whole card, once every question has a choice or
//! words: the daemon sends the agent one message quoting each question with its answer under it
//! (`agent.answer`). Once the agent took it, the card folds to its answers: each question on one
//! line, what the user chose and wrote under it.

use std::collections::HashSet;

use flottille_core::question::{Answer, Question};
use gpui_design_system::{
    Button, Card, Chip, Field, Toast, palette, presence, quoted_title, space, toast,
};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement as _, Styled,
    Subscription, TestSupportExt as _, WeakEntity, Window, div, px,
};

use crate::session::{MarkdownText, SessionPage};

/// What a question's card says once its answer was sent while the agent works: a message of the
/// user waits for the end of the agent's turn (or "Send now" under the conversation), and the
/// card folds to its answers once the agent took it.
pub const ANSWER_WAITS: &str =
    "Your answer is sent: it waits under the conversation until the agent's turn ends.";

/// The most lines a field grows to before it scrolls.
const MOST_LINES: usize = 8;

/// The asks whose answer the user sent, by session: the window keeps them, so that a page opened
/// again, or a history read again, does not ask for an answer twice.
#[derive(Default)]
struct SentAnswers(HashSet<(String, u64)>);

impl gpui_kit::Global for SentAnswers {}

/// The answer to the ask `seq` of `session` was taken by the daemon.
pub fn answers_sent(session: &str, seq: u64, cx: &mut App) {
    let sent = &mut cx.default_global::<SentAnswers>().0;
    sent.insert((session.to_owned(), seq));
}

/// What a window that was started again knows: nothing but what the daemon's history says.
pub fn forget_sent_answers(cx: &mut App) {
    cx.default_global::<SentAnswers>().0.clear();
}

/// The blocks of an ask's card, in their own view: a caret blinks without drawing the row.
pub struct QuestionCard {
    at: usize,
    seq: u64,
    session: String,
    page: WeakEntity<SessionPage>,
    questions: Vec<Question>,
    texts: Vec<MarkdownText>,
    fields: Vec<Entity<TextareaState>>,
    /// The options chosen, by question.
    chosen: Vec<Vec<String>>,
    sending: bool,
    /// The history says the ask was answered (`agent.answered`): true after a restart too.
    answered: bool,
    _subscriptions: Vec<Subscription>,
}

impl QuestionCard {
    pub fn new(
        at: usize,
        seq: u64,
        session: String,
        questions: Vec<Question>,
        page: WeakEntity<SessionPage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        let fields = (0..questions.len())
            .map(|_| {
                let field = cx.new(|cx| {
                    let mut field = TextareaState::new(window, cx)
                        .auto_grow(1, MOST_LINES)
                        .placeholder("Your answer");
                    gpui_design_system::follow_theme(&mut field, cx);
                    field
                });
                subscriptions.push(cx.subscribe_in(
                    &field,
                    window,
                    move |this: &mut Self, _, event, _, cx| match event {
                        InputEvent::Change => cx.notify(),
                        // `Enter` breaks the line in the field; `Ctrl+Enter` sends.
                        InputEvent::PressEnter {
                            secondary: true, ..
                        } => this.send(cx),
                        _ => {}
                    },
                ));
                field
            })
            .collect();
        subscriptions.push(cx.observe_global::<SentAnswers>(|_, cx| cx.notify()));
        let texts = questions
            .iter()
            .map(|q| MarkdownText::new(&q.question, cx))
            .collect();
        QuestionCard {
            at,
            seq,
            session,
            page,
            chosen: vec![Vec::new(); questions.len()],
            questions,
            texts,
            fields,
            sending: false,
            answered: false,
            _subscriptions: subscriptions,
        }
    }

    /// The field of the words of question `n` (from 0).
    pub fn field(&self, n: usize) -> &Entity<TextareaState> {
        &self.fields[n]
    }

    /// Picks the option `label` of question `n`, or lets it go: a question of one choice keeps
    /// the last one picked.
    pub fn choose(&mut self, n: usize, label: &str, cx: &mut Context<Self>) {
        let multiple = self.questions[n].multiple;
        let chosen = &mut self.chosen[n];
        match chosen.iter().position(|c| c == label) {
            Some(at) => {
                chosen.remove(at);
            }
            None if multiple => chosen.push(label.to_owned()),
            None => *chosen = vec![label.to_owned()],
        }
        // In the order the agent gave them.
        let order: Vec<&str> = self.questions[n]
            .options
            .iter()
            .map(|o| o.label.as_str())
            .collect();
        chosen.sort_by_key(|c| order.iter().position(|o| o == c));
        cx.notify();
    }

    /// The answers as the card holds them, one per question.
    pub fn answers(&self, cx: &App) -> Vec<Answer> {
        self.fields
            .iter()
            .zip(&self.chosen)
            .map(|(field, chosen)| Answer {
                choices: chosen.clone(),
                text: field.read(cx).value().trim().to_owned(),
            })
            .collect()
    }

    /// Every question has a choice or words.
    pub fn is_ready(&self, cx: &App) -> bool {
        self.answers(cx).iter().all(|answer| !answer.is_empty())
    }

    /// Whether the answer went: the fields give way to a line saying it waits. The window's
    /// memory says it, and so does the ask's history once the answers were kept with it.
    pub fn is_sent(&self, cx: &App) -> bool {
        self.answered
            || cx
                .try_global::<SentAnswers>()
                .is_some_and(|sent| sent.0.contains(&(self.session.clone(), self.seq)))
    }

    /// Whether the ask's history holds the answers given from this card.
    pub fn set_answered(&mut self, answered: bool) {
        self.answered = answered;
    }

    /// Sends the answers to the root, once every question has one.
    pub fn send(&mut self, cx: &mut Context<Self>) {
        if !self.is_ready(cx) || self.sending || self.is_sent(cx) {
            return;
        }
        self.sending = true;
        let (seq, answers) = (self.seq, self.answers(cx));
        self.page
            .update(cx, |page, cx| page.answer_question(seq, answers, cx))
            .ok();
        cx.notify();
    }

    /// What the daemon answered to the answers sent.
    pub fn answered(&mut self, refused: Option<String>, cx: &mut Context<Self>) {
        self.sending = false;
        if let Some(why) = refused {
            toast(Toast::failure("The answer was not sent").detail(why), cx);
        }
        cx.notify();
    }

    fn block(&self, n: usize, open: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let at = self.at;
        let id = |part: String| ElementId::Name(format!("row-{at}-{part}").into());
        let question = &self.questions[n];
        let chips = question.options.iter().enumerate().map(|(j, option)| {
            let label = option.label.clone();
            let chosen = self.chosen[n].contains(&label);
            let chip = Chip::choice(
                id(format!("question-{n}-option-{j}")),
                label.clone(),
                chosen,
            )
            .on_click({
                let card = cx.entity().downgrade();
                move |_, _, cx| {
                    card.update(cx, |card, cx| card.choose(n, &label, cx)).ok();
                }
            });
            match &option.description {
                Some(description) => chip.faint(format!("· {description}")),
                None => chip,
            }
        });
        let frame = div()
            .id(id(format!("answer-{n}-frame")))
            .px(space::MD)
            .py(space::XS)
            .rounded(px(8.))
            .bg(p.field)
            .border_1()
            .border_color(p.line2)
            .text_size(px(13.))
            .line_height(px(20.))
            .child(self.fields[n].clone())
            .test_support();
        let label = match question.options.is_empty() {
            true => "Your answer",
            false => "Your words, with the choice or instead",
        };
        let inputs = div()
            .flex()
            .flex_col()
            .gap(space::SM)
            .when(!question.options.is_empty(), |d| {
                d.child(
                    div()
                        .id(id(format!("question-{n}-options")))
                        .flex()
                        .flex_wrap()
                        .gap(space::SM)
                        .children(chips)
                        .test_support(),
                )
            })
            .child(Field::new(
                id(format!("answer-{n}-field")),
                label,
                gpui_design_system::growing(id(format!("answer-{n}-height")), frame),
            ));
        div()
            .id(id(format!("question-{n}")))
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(
                div()
                    .text_size(px(13.5))
                    .child(self.texts[n].view(id(format!("question-{n}-text")))),
            )
            .child(presence(id(format!("question-{n}-inputs")), open, inputs))
            .test_support()
    }
}

impl Render for QuestionCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let at = self.at;
        let id = |part: &str| ElementId::Name(format!("row-{at}-{part}").into());
        let sent = self.is_sent(cx);
        let blocks: Vec<AnyElement> = (0..self.questions.len())
            .map(|n| self.block(n, !sent, cx).into_any_element())
            .collect();
        let send = div().flex().justify_end().child(
            Button::primary(id("answer-send"), "Answer")
                .disabled(!self.is_ready(cx) || self.sending)
                .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
        );
        let waits = div()
            .id(id("answer-waits"))
            .text_size(px(12.5))
            .text_color(p.faint)
            .aria_label(ANSWER_WAITS)
            .child(ANSWER_WAITS)
            .test_support();
        div()
            .id(id("question-text"))
            .flex()
            .flex_col()
            .gap(space::LG)
            .children(blocks)
            .child(presence(id("answer-open"), !sent, send))
            .child(presence(id("answer-sent"), sent, waits))
            .test_support()
    }
}

/// The card of an ask while it waits: who asks, by the session's title, and its blocks.
pub fn open_card(at: usize, title: &str, when: SharedString, card: Entity<QuestionCard>) -> Card {
    Card::new()
        .title(heading(
            at,
            "question-title",
            format!("{} asks you", quoted_title(title)),
        ))
        .meta(when)
        .child(div().px(space::LG).py(space::MD).child(card))
}

/// What a card of a led session's question says under its title: who answers it, and that the
/// user may too.
pub const ASKED_NOTE: &str = "Its pilot answers at the end of its turn; you can answer here too.";

/// A card's title, named for who reads it.
pub(crate) fn heading(at: usize, part: &str, said: String) -> impl IntoElement {
    let said = SharedString::from(said);
    div()
        .id(ElementId::Name(format!("row-{at}-{part}").into()))
        .aria_label(said.clone())
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(said)
        .test_support()
}

/// The card of an ask once the agent took its answers, folded to them: each question on one
/// line, what the user chose and wrote under it.
pub fn answered_card(
    at: usize,
    title: &str,
    when: SharedString,
    questions: &[Question],
    answers: &[Answer],
    cx: &App,
) -> Card {
    let p = palette(cx);
    let id = |part: String| ElementId::Name(format!("row-{at}-{part}").into());
    let lines = questions
        .iter()
        .zip(answers)
        .enumerate()
        .map(|(n, (q, a))| {
            let asked = q.question.lines().next().unwrap_or_default().to_owned();
            let mut said: Vec<String> = Vec::new();
            if !a.choices.is_empty() {
                said.push(a.choices.join(", "));
            }
            if !a.text.is_empty() {
                said.push(a.text.clone());
            }
            let said = said.join("\n");
            div()
                .id(id(format!("answered-{n}")))
                .flex()
                .flex_col()
                .gap(space::XS)
                .child(
                    div()
                        .id(id(format!("answered-{n}-question")))
                        .aria_label(SharedString::from(asked.clone()))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child(SharedString::from(asked))
                        .test_support(),
                )
                .child(
                    div()
                        .id(id(format!("answered-{n}-answer")))
                        .aria_label(SharedString::from(said.clone()))
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(p.text)
                        .children(
                            said.lines()
                                .map(|line| div().child(line.to_owned()))
                                .collect::<Vec<_>>(),
                        )
                        .test_support(),
                )
                .test_support()
        });
    Card::new()
        .title(heading(
            at,
            "answered-title",
            format!("{} asked you", quoted_title(title)),
        ))
        .meta(when)
        .child(
            div()
                .px(space::LG)
                .py(space::MD)
                .flex()
                .flex_col()
                .gap(space::MD)
                .children(lines),
        )
}
