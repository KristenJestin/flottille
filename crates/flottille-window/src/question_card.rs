//! The card of an ask of the root to the user (`docs/tools.md`, "ask"), docked above the
//! composer while it waits (`ask_dock.rs`): who asks, by the session's title, an `asks you` tag
//! and how many questions are answered, then one question at a time (`2 of 4`, Previous and
//! Next, `←`/`→`, a dot per question), each with its options as rows (a round mark for one choice, a square one for
//! several) and, as the last row, "Something else…", which holds its field of several lines,
//! right of its mark, once it is picked. Once an option is chosen, "+ Add a note" unfolds a field
//! for a note with the choice (`Enter` breaks a line, `Ctrl+Enter` sends). One Answer for the
//! whole card, once every question has a choice or words: the daemon sends the agent one message
//! quoting each question with its answer under it (`agent.answer`). Once the agent took it, the
//! card folds to its answers: each question on one line, what the user chose and wrote under it.

use std::collections::HashSet;
use std::time::Duration;

use flottille_core::question::{Answer, Question};
use gpui_design_system::{
    Block, Button, Card, ChoiceRow, Dots, Field, Icon, Toast, Type as _, changing, fade, icon,
    palette, presence, quoted_title, sliding, space, toast,
};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _,
    FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, Task, TestSupportExt as _,
    WeakEntity, Window, div, px,
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

/// The ask's card, in its own view: a caret blinks without drawing the row.
pub struct QuestionCard {
    at: usize,
    seq: u64,
    session: String,
    page: WeakEntity<SessionPage>,
    /// The session's title and the time of the ask, which the row says.
    title: String,
    when: SharedString,
    questions: Vec<Question>,
    texts: Vec<MarkdownText>,
    /// The words of "Something else…", or of a question with no options.
    fields: Vec<Entity<TextareaState>>,
    /// The note that goes with a choice.
    notes: Vec<Entity<TextareaState>>,
    /// The options chosen, by question.
    chosen: Vec<Vec<String>>,
    /// "Something else…" is picked.
    other: Vec<bool>,
    /// "+ Add a note" was clicked.
    noting: Vec<bool>,
    sending: bool,
    /// What takes `Ctrl+Enter` when no field has the focus: a card of options alone has none
    /// open, and a click on a row puts the focus here.
    focus: FocusHandle,
    /// The history says the ask was answered (`agent.answered`): true after a restart too.
    answered: bool,
    /// The question shown, from 0.
    current: usize,
    /// The card was asked for from the conversation: its ring lights once, then fades.
    pulsing: bool,
    _pulse: Option<Task<()>>,
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
        let mut make =
            |placeholder: Option<&'static str>, window: &mut Window, cx: &mut Context<Self>| {
                let field = cx.new(|cx| {
                    let mut field = TextareaState::new(window, cx).auto_grow(1, MOST_LINES);
                    if let Some(placeholder) = placeholder {
                        field = field.placeholder(placeholder);
                    }
                    gpui_design_system::follow_theme(&mut field, cx);
                    field
                });
                subscriptions.push(cx.subscribe_in(
                    &field,
                    window,
                    move |this: &mut Self, _, event, window, cx| match event {
                        InputEvent::Change => cx.notify(),
                        // `Enter` breaks the line in the field; `Ctrl+Enter` sends, and the
                        // field that gives way with the card does not keep the focus.
                        InputEvent::PressEnter {
                            secondary: true, ..
                        } => {
                            this.send(cx);
                            if this.sending {
                                window.focus(&this.focus, cx);
                            }
                        }
                        _ => {}
                    },
                ));
                field
            };
        let fields: Vec<_> = questions
            .iter()
            .map(|q| {
                let placeholder = (!q.options.is_empty()).then_some("Say what you want instead");
                make(placeholder, window, cx)
            })
            .collect();
        let notes: Vec<_> = questions
            .iter()
            .map(|_| make(Some("Optional"), window, cx))
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
            title: String::new(),
            when: SharedString::default(),
            other: vec![false; questions.len()],
            noting: vec![false; questions.len()],
            chosen: vec![Vec::new(); questions.len()],
            questions,
            texts,
            fields,
            notes,
            sending: false,
            focus: cx.focus_handle(),
            answered: false,
            current: 0,
            pulsing: false,
            _pulse: None,
            _subscriptions: subscriptions,
        }
    }

    /// The session's title and the time of the ask, as the row says them.
    pub fn set_header(&mut self, title: &str, when: SharedString, cx: &mut Context<Self>) {
        if self.title != title || self.when != when {
            self.title = title.to_owned();
            self.when = when;
            cx.notify();
        }
    }

    /// The field of the words of question `n` (from 0): "Something else…", or the whole answer
    /// of a question with no options.
    pub fn field(&self, n: usize) -> &Entity<TextareaState> {
        &self.fields[n]
    }

    /// The row of the conversation where it was asked.
    pub fn at(&self) -> usize {
        self.at
    }

    /// The ask's number in the history.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The question shown, from 0.
    pub fn current(&self) -> usize {
        self.current
    }

    /// Shows question `n` (from 0): it slides in from the side of the move.
    pub fn go(&mut self, n: usize, cx: &mut Context<Self>) {
        let n = n.min(self.questions.len().saturating_sub(1));
        if self.current != n {
            self.current = n;
            cx.notify();
        }
    }

    /// Brings the focus to the card, which lights its ring once.
    pub fn summon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.pulsing = true;
        self._pulse = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(gpui_design_system::motion::PANEL + Duration::from_millis(120))
                .await;
            this.update(cx, |this, cx| {
                this.pulsing = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Its ring is lit: the line in the conversation asked for it.
    pub fn pulsing(&self) -> bool {
        self.pulsing
    }

    /// Whether the card itself holds the focus (what takes `Ctrl+Enter` when no field has it).
    pub fn has_focus(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// The field of the note that goes with the choice of question `n`.
    pub fn note(&self, n: usize) -> &Entity<TextareaState> {
        &self.notes[n]
    }

    /// Picks the option `label` of question `n`, or lets it go: a question of one choice keeps
    /// the last one picked, "Something else…" included.
    pub fn choose(&mut self, n: usize, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let multiple = self.questions[n].multiple;
        let chosen = &mut self.chosen[n];
        match chosen.iter().position(|c| c == label) {
            Some(at) => {
                chosen.remove(at);
            }
            None if multiple => chosen.push(label.to_owned()),
            None => {
                *chosen = vec![label.to_owned()];
                self.other[n] = false;
            }
        }
        // In the order the agent gave them.
        let order: Vec<&str> = self.questions[n]
            .options
            .iter()
            .map(|o| o.label.as_str())
            .collect();
        chosen.sort_by_key(|c| order.iter().position(|o| o == c));
        self.drop_note_without_choice(n, window, cx);
        // One choice is the whole answer: the next question waiting for one comes.
        if !multiple && !self.chosen[n].is_empty() {
            let len = self.questions.len();
            let next = (n + 1..len).chain(0..n).find(|&m| !self.is_answered(m, cx));
            if let Some(next) = next {
                self.current = next;
            }
        }
        cx.notify();
    }

    /// A note goes with a choice: with none left on question `n` it folds away, empty, and
    /// "+ Add a note" comes back with the next choice.
    fn drop_note_without_choice(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.chosen[n].is_empty() {
            return;
        }
        self.noting[n] = false;
        self.notes[n].update(cx, |note, cx| note.set_value("", window, cx));
    }

    /// Picks "Something else…" of question `n`, or lets it go: in a question of one choice it
    /// takes the place of the option chosen.
    pub fn choose_other(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.other[n] = !self.other[n];
        if self.other[n] && !self.questions[n].multiple {
            self.chosen[n].clear();
            self.drop_note_without_choice(n, window, cx);
        }
        cx.notify();
    }

    /// Whether the words of question `n` are asked: "Something else…" is picked, or there is
    /// nothing else to pick.
    fn words_open(&self, n: usize) -> bool {
        self.questions[n].options.is_empty() || self.other[n]
    }

    /// Whether the note of question `n` is open: an option is chosen, and the button was clicked.
    fn note_open(&self, n: usize) -> bool {
        self.can_note(n) && self.noting[n]
    }

    /// Whether question `n` may have a note: an option is chosen and the words of "Something
    /// else…" are not.
    fn can_note(&self, n: usize) -> bool {
        !self.chosen[n].is_empty() && !self.other[n]
    }

    /// The answers as the card holds them, one per question: the choices, and the words that
    /// were asked ("Something else…") or the note that was opened.
    pub fn answers(&self, cx: &App) -> Vec<Answer> {
        (0..self.questions.len())
            .map(|n| {
                let words =
                    |field: &Entity<TextareaState>| field.read(cx).value().trim().to_owned();
                let text = if self.words_open(n) {
                    words(&self.fields[n])
                } else if self.note_open(n) {
                    words(&self.notes[n])
                } else {
                    String::new()
                };
                Answer {
                    choices: self.chosen[n].clone(),
                    text,
                }
            })
            .collect()
    }

    /// How many questions have a choice or words: "Something else…" always needs its words, with
    /// another choice beside it or not.
    pub fn answered_count(&self, cx: &App) -> usize {
        (0..self.questions.len())
            .filter(|&n| self.is_answered(n, cx))
            .count()
    }

    /// Whether question `n` has a choice or words.
    fn is_answered(&self, n: usize, cx: &App) -> bool {
        let answer = &self.answers(cx)[n];
        if self.words_open(n) {
            !answer.text.is_empty()
        } else {
            !answer.is_empty()
        }
    }

    /// Every question has a choice or words.
    pub fn is_ready(&self, cx: &App) -> bool {
        self.answered_count(cx) == self.questions.len()
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
        let card = cx.entity().downgrade();
        let rows = question.options.iter().enumerate().map(|(j, option)| {
            let label = option.label.clone();
            let mut row = ChoiceRow::new(id(format!("question-{n}-option-{j}")), label.clone())
                .chosen(self.chosen[n].contains(&label))
                .on_click({
                    let card = card.clone();
                    move |window, cx| {
                        card.update(cx, |card, cx| {
                            card.choose(n, &label, window, cx);
                            window.focus(&card.focus, cx);
                        })
                        .ok();
                    }
                });
            if question.multiple {
                row = row.square();
            }
            match &option.description {
                Some(description) => row.description(description.clone()),
                None => row,
            }
        });
        // The words of "Something else…" are its field, in its row: the row's words are its label,
        // so the field is named for readers of the screen only.
        let inline = {
            let frame = div()
                .id(id(format!("answer-{n}-frame")))
                .aria_label("Your answer")
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
            gpui_design_system::growing(id(format!("answer-{n}-height")), frame)
        };
        let other = (!question.options.is_empty()).then(|| {
            let mut row = ChoiceRow::new(id(format!("question-{n}-other")), "Something else…")
                .faint()
                .chosen(self.other[n])
                .field("Something else:", inline)
                .field_inset(px(5.))
                .on_click({
                    let card = card.clone();
                    move |window, cx| {
                        card.update(cx, |card, cx| {
                            card.choose_other(n, window, cx);
                            // The words are asked at once; without them, `Ctrl+Enter` still sends.
                            let focus = if card.other[n] {
                                card.fields[n].read(cx).focus_handle(cx)
                            } else {
                                card.focus.clone()
                            };
                            window.focus(&focus, cx);
                        })
                        .ok();
                    }
                });
            if question.multiple {
                row = row.square();
            }
            row
        });
        let field = |part: &str, label: &'static str, field: &Entity<TextareaState>| {
            let frame = div()
                .id(id(format!("{part}-{n}-frame")))
                .px(space::MD)
                .py(space::XS)
                .rounded(px(8.))
                .bg(p.field)
                .border_1()
                .border_color(p.line2)
                .text_size(px(13.))
                .line_height(px(20.))
                .child(field.clone())
                .test_support();
            div().ml(space::XXL).child(Field::new(
                id(format!("{part}-{n}-field")),
                label,
                gpui_design_system::growing(id(format!("{part}-{n}-height")), frame),
            ))
        };
        let several = question.multiple.then(|| {
            div()
                .id(id(format!("question-{n}-several")))
                .aria_label("several")
                .flex_none()
                .ml_auto()
                .whitespace_nowrap()
                .text_size(px(12.))
                .text_color(p.faint)
                .child("several")
                .test_support()
        });
        let top = div()
            .flex()
            .items_start()
            .gap(space::SM)
            .child(
                div()
                    .flex_none()
                    .w(space::XL)
                    .mono(12.)
                    .text_color(p.faint)
                    .child(format!("{}", n + 1)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.texts[n].view(id(format!("question-{n}-text")))),
            )
            .children(several);
        let options = div()
            .id(id(format!("question-{n}-options")))
            .ml(space::XXL)
            .flex()
            .flex_col()
            .gap(space::XS)
            .children(rows)
            .children(other)
            .test_support();
        let inputs = div()
            .flex()
            .flex_col()
            .gap(space::SM)
            .when(!question.options.is_empty(), |d| d.child(options))
            .child(
                presence(
                    id(format!("question-{n}-words")),
                    open && question.options.is_empty(),
                    field("answer", "Your answer", &self.fields[n]),
                )
                .gap(space::SM),
            )
            .child(
                presence(
                    id(format!("note-{n}-open")),
                    open && self.note_open(n),
                    field("note", "Note with your choice", &self.notes[n]),
                )
                .gap(space::SM),
            )
            .child(
                presence(
                    id(format!("note-{n}-ask")),
                    open && self.can_note(n) && !self.noting[n],
                    div().ml(space::XXL).flex().child(
                        Button::quiet(id(format!("note-{n}-add")), "+ Add a note").on_click({
                            let card = card.clone();
                            move |_, _, cx| {
                                card.update(cx, |card, cx| {
                                    card.noting[n] = true;
                                    cx.notify();
                                })
                                .ok();
                            }
                        }),
                    ),
                )
                .gap(space::SM),
            );
        div()
            .id(id(format!("question-{n}")))
            .px(space::LG)
            .py(space::MD)
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(top)
            .child(presence(id(format!("question-{n}-inputs")), open, inputs))
            .test_support()
    }

    /// "n of m answered", faint: it fades back in when the count changes.
    fn count(&self, cx: &App) -> impl IntoElement {
        let p = palette(cx);
        let id = ElementId::Name(format!("row-{}-question-count", self.at).into());
        let said = format!(
            "{} of {} answered",
            self.answered_count(cx),
            self.questions.len()
        );
        let fade = ElementId::Name(format!("row-{}-question-count-fade", self.at).into());
        changing(fade, said, move |text| {
            div()
                .id(id)
                .aria_label(text.clone())
                .flex_none()
                .whitespace_nowrap()
                .text_size(px(12.))
                .text_color(p.faint)
                .child(text)
                .test_support()
                .into_any_element()
        })
    }
}

impl Render for QuestionCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let at = self.at;
        let id = |part: &str| ElementId::Name(format!("row-{at}-{part}").into());
        let sent = self.is_sent(cx);
        let ready = self.is_ready(cx);
        let mut card = Card::new()
            .title(head(
                at,
                "question",
                &self.title,
                "asks you",
                (p.wait, p.wait_bg),
                Some(self.count(cx).into_any_element()),
            ))
            .meta(self.when.clone());
        // The steps first: in a dock too short for the whole card, they stay in sight.
        if self.questions.len() > 1 {
            card = card.block(Block::new().child(self.steps(cx)));
        }
        let current = self.current;
        let question = self.block(current, !sent, cx);
        card = card.block(Block::new().child(sliding(id("question-page"), current, question)));
        let why = if ready {
            "Sent to the agent as one message."
        } else {
            "Answer each question to send."
        };
        let why = changing(id("answer-why-fade"), why, {
            let id = id("answer-why");
            move |text| {
                div()
                    .id(id)
                    .aria_label(text.clone())
                    .text_size(px(12.5))
                    .text_color(p.faint)
                    .child(text)
                    .test_support()
                    .into_any_element()
            }
        });
        let send = div()
            .flex()
            .items_center()
            .gap(space::MD)
            .child(div().flex_1().min_w_0().child(why))
            .child(
                div()
                    .id(id("answer-keys"))
                    .aria_label("Ctrl+Enter")
                    .flex_none()
                    .mono(11.5)
                    .text_color(p.faint)
                    .child("Ctrl+Enter")
                    .test_support(),
            )
            .child(
                Button::primary(id("answer-send"), "Answer")
                    .disabled(!ready || self.sending)
                    .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
            );
        let waits = div()
            .id(id("answer-waits"))
            .text_size(px(12.5))
            .text_color(p.faint)
            .aria_label(ANSWER_WAITS)
            .child(ANSWER_WAITS)
            .test_support();
        let footer = div()
            .px(space::LG)
            .py(space::MD)
            .child(presence(id("answer-open"), !sent, send))
            .child(presence(id("answer-sent"), sent, waits));
        card = card.block(Block::new().child(footer));
        let ring = fade(id("question-pulse"), self.pulsing, PULSE, window, cx);
        div()
            .id(id("question-text"))
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let keys = &event.keystroke;
                if keys.key == "enter" && keys.modifiers.secondary() {
                    cx.stop_propagation();
                    this.send(cx);
                }
                // The arrows move between questions only from the card itself: in a field they
                // move the caret.
                if this.focus.is_focused(window) && !keys.modifiers.modified() {
                    match keys.key.as_str() {
                        "left" => this.go(this.current.saturating_sub(1), cx),
                        "right" => this.go(this.current + 1, cx),
                        _ => {}
                    }
                }
            }))
            .child(card)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(12.))
                    .border_2()
                    .border_color(p.focus_ring.opacity(ring)),
            )
            .test_support()
    }
}

impl gpui_kit::Focusable for QuestionCard {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// How long the ring of a card asked for from the conversation takes to light and to fade.
const PULSE: Duration = gpui_design_system::motion::PANEL;

impl QuestionCard {
    /// Above the question: Previous, `2 of 4` and a dot per question, Next.
    fn steps(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let at = self.at;
        let id = |part: &str| ElementId::Name(format!("row-{at}-{part}").into());
        let (current, len) = (self.current, self.questions.len());
        let mut dots = Dots::new(id("question-dots"), "Questions").current(current);
        for n in 0..len {
            dots = dots.dot(self.is_answered(n, cx));
        }
        let step = changing(
            id("question-step-fade"),
            format!("{} of {len}", current + 1),
            {
                let id = id("question-step");
                move |text| {
                    div()
                        .id(id)
                        .aria_label(text.clone())
                        .flex_none()
                        .whitespace_nowrap()
                        .mono(11.5)
                        .text_color(p.faint)
                        .child(text)
                        .test_support()
                        .into_any_element()
                }
            },
        );
        div()
            .px(space::LG)
            .py(space::SM)
            .flex()
            .items_center()
            .gap(space::MD)
            .child(
                Button::quiet(id("question-previous"), "Previous")
                    .disabled(current == 0)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.go(this.current.saturating_sub(1), cx)),
                    ),
            )
            .child(div().flex_1())
            .child(step)
            .child(dots)
            .child(div().flex_1())
            .child(
                Button::quiet(id("question-next"), "Next")
                    .disabled(current + 1 >= len)
                    .on_click(cx.listener(|this, _, _, cx| this.go(this.current + 1, cx))),
            )
    }
}

/// The header of a card of an ask: the session's title, a tag saying what the card is, what it
/// counts, if anything. `part` names its title (`<part>-title`) and its tag (`<part>-tag`).
fn head(
    at: usize,
    part: &str,
    title: &str,
    tag: &'static str,
    (ink, fill): (Hsla, Hsla),
    count: Option<AnyElement>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(space::SM)
        .w_full()
        .child(div().flex_1().min_w_0().child(heading(
            at,
            &format!("{part}-title"),
            quoted_title(title),
        )))
        .child(
            div()
                .id(ElementId::Name(format!("row-{at}-{part}-tag").into()))
                .aria_label(tag)
                .flex_none()
                .px(space::SM)
                .rounded_full()
                .bg(fill)
                .text_color(ink)
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .child(tag)
                .test_support(),
        )
        .children(count)
}

/// What a card of a led session's question says under its title: who answers it, and that the
/// user may too.
pub const ASKED_NOTE: &str = "Its lead answers at the end of its turn; you can answer here too.";

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
/// line, a check and what the user chose under it, the note under that.
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
    let mut card = Card::new()
        .title(head(
            at,
            "answered",
            title,
            "answered",
            (p.ok, p.ok_bg),
            None,
        ))
        .meta(when);
    for (n, (q, a)) in questions.iter().zip(answers).enumerate() {
        let asked = q.question.lines().next().unwrap_or_default().to_owned();
        let mut said: Vec<String> = Vec::new();
        if !a.choices.is_empty() {
            said.push(a.choices.join(", "));
        }
        if !a.text.is_empty() {
            said.push(a.text.clone());
        }
        let said = said.join("\n");
        // What was chosen is the answer; the words under a choice are its note, and words
        // alone are the answer.
        let (headline, note) = match a.choices.is_empty() {
            true => (a.text.clone(), String::new()),
            false => (a.choices.join(", "), a.text.clone()),
        };
        let lines = |text: &str, ink, weight| {
            div()
                .flex()
                .flex_col()
                .text_size(px(13.))
                .font_weight(weight)
                .text_color(ink)
                .children(
                    text.lines()
                        .map(|line| div().child(line.to_owned()))
                        .collect::<Vec<_>>(),
                )
        };
        card =
            card.block(
                Block::new().child(
                    div()
                        .id(id(format!("answered-{n}")))
                        .px(space::LG)
                        .py(space::MD)
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
                                .text_size(px(13.))
                                .text_color(p.mute)
                                .child(SharedString::from(format!("{} · {asked}", n + 1)))
                                .test_support(),
                        )
                        .child(
                            div()
                                .id(id(format!("answered-{n}-answer")))
                                .aria_label(SharedString::from(said.clone()))
                                .flex()
                                .flex_col()
                                .gap(space::XS)
                                .child(
                                    div()
                                        .flex()
                                        .items_start()
                                        .gap(space::SM)
                                        .child(div().flex_none().pt(space::XS).child(
                                            icon(Icon::Check).size(px(13.)).text_color(p.ok),
                                        ))
                                        .child(lines(&headline, p.text, FontWeight::MEDIUM)),
                                )
                                .when(!note.is_empty(), |d| {
                                    d.child(div().ml(space::XL).child(lines(
                                        &note,
                                        p.mute,
                                        FontWeight::NORMAL,
                                    )))
                                })
                                .test_support(),
                        )
                        .test_support(),
                ),
            );
    }
    card
}
