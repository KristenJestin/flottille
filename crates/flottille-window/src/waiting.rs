//! The rail's "Waiting for you" section: each request that reached the user is a card with Allow
//! and Deny, above everything else in the rail. The conversation keeps one compact line per
//! request that scrolls here; an answered request leaves the rail and its line says the outcome.
//!
//! The text of a card is what the daemon gave (`says`, the class, the judge's reason, the reasons
//! it was held, who asked); the window only joins the reasons into sentences.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use flottille_ui::motion::PANEL;
use flottille_ui::{
    Button, Card, Item, Listing, Palette, Placement, Type, fade, palette, presence, section_label,
    space,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Task, TestSupportExt as _, WeakEntity,
    Window, div, px,
};

use crate::actions::Answer;
use crate::session::{ApprovalCard, BASE_SAYS, SessionPage};

/// How long a card the user was sent to stays lit.
const POINTED: Duration = Duration::from_millis(1600);

/// The reasons a call was held, each as a sentence of its own: the first letter in capital, a
/// full stop at the end, a space between. The daemon gives them as fragments, which read as one
/// run of words (a word doubled where one ends and the next begins) when only spaced.
pub fn sentences<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let first = chars.next().map(|c| c.to_uppercase().collect::<String>());
            let mut sentence = first.unwrap_or_default() + chars.as_str();
            if !sentence.ends_with(['.', '!', '?']) {
                sentence.push('.');
            }
            sentence
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct Waiting {
    page: WeakEntity<SessionPage>,
    /// The requests that wait, in the order they were asked.
    cards: Vec<ApprovalCard>,
    list: Listing<ApprovalCard>,
    /// Answers sent and not yet taken.
    busy: HashSet<u64>,
    /// What the daemon said when it refused an answer.
    errors: HashMap<u64, SharedString>,
    /// The card the user was sent to from the conversation: lit for a moment.
    pointed: Option<u64>,
    _lit: Option<Task<()>>,
    renders: usize,
}

impl Waiting {
    pub fn new(page: WeakEntity<SessionPage>) -> Self {
        Self {
            page,
            cards: Vec::new(),
            list: Listing::default(),
            busy: HashSet::new(),
            errors: HashMap::new(),
            pointed: None,
            _lit: None,
            renders: 0,
        }
    }

    /// The requests that wait for the user.
    pub fn cards(&self) -> &[ApprovalCard] {
        &self.cards
    }

    pub fn renders(&self) -> usize {
        self.renders
    }

    /// The card the user was sent to, while it is lit.
    pub fn pointed(&self) -> Option<u64> {
        self.pointed
    }

    /// The requests that wait now; nothing is drawn again when they are what was drawn.
    pub fn set(&mut self, cards: Vec<ApprovalCard>, cx: &mut Context<Self>) {
        if self.cards == cards {
            return;
        }
        self.busy.retain(|n| cards.iter().any(|c| c.number == *n));
        self.errors
            .retain(|n, _| cards.iter().any(|c| c.number == *n));
        self.cards = cards;
        cx.notify();
    }

    /// The daemon's answer to Allow or Deny on a card: it leaves with the request, or says why not.
    pub fn answered(&mut self, number: u64, refused: Option<String>, cx: &mut Context<Self>) {
        self.busy.remove(&number);
        match refused {
            Some(why) => self.errors.insert(number, why.into()),
            None => self.errors.remove(&number),
        };
        cx.notify();
    }

    /// Lights a card for a moment: the user was sent to it from the conversation.
    pub fn point_at(&mut self, number: u64, cx: &mut Context<Self>) {
        self.pointed = Some(number);
        self._lit = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(POINTED).await;
            this.update(cx, |this, cx| {
                if this.pointed == Some(number) {
                    this.pointed = None;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn answer(&mut self, number: u64, answer: Answer, cx: &mut Context<Self>) {
        if !self.busy.insert(number) {
            return;
        }
        self.errors.remove(&number);
        self.page
            .update(cx, |page, cx| page.answer(number, answer, cx))
            .ok();
        cx.notify();
    }

    fn card(
        &self,
        card: &ApprovalCard,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let number = card.number;
        let id = |part: &str| ElementId::Name(format!("{part}-{number}").into());
        let busy = self.busy.contains(&number);
        let lit = fade(
            id("approval-lit"),
            self.pointed == Some(number),
            PANEL,
            window,
            cx,
        );
        let button = |part: &str, label: &str, answer: Answer, primary: bool| {
            let button = match primary {
                true => Button::primary(id(part), label.to_owned()),
                false => Button::quiet(id(part), label.to_owned()),
            };
            button
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.answer(number, answer, cx)))
        };
        let class = if card.base { "Base" } else { "To judge" };
        let body = div()
            .px(space::MD)
            .pt(space::MD)
            .flex()
            .flex_col()
            .gap(space::XS)
            .child(
                div()
                    .id(id("approval-title"))
                    .text_size(px(13.5))
                    .semibold()
                    .child(SharedString::from(format!(
                        "{}?",
                        card.asked
                            .clone()
                            .unwrap_or_else(|| format!("Request #{number}"))
                    ))),
            )
            .children(card.says.iter().enumerate().map(|(i, said)| {
                div()
                    .id(ElementId::Name(
                        format!("approval-says-{number}-{i}").into(),
                    ))
                    .text_size(px(12.5))
                    .child(SharedString::from(said.clone()))
                    .test_support()
            }))
            .child(
                div()
                    .id(id("approval-class"))
                    .aria_label(class)
                    .mono(11.)
                    .text_color(if card.base { p.fail } else { p.wait })
                    .child(class)
                    .test_support(),
            )
            .when(card.base, |d| {
                d.child(
                    div()
                        .id(id("approval-base"))
                        .aria_label(BASE_SAYS)
                        .text_size(px(12.5))
                        .semibold()
                        .text_color(p.fail)
                        .child(BASE_SAYS)
                        .test_support(),
                )
            })
            .when_some(card.judgement.clone(), |d, judgement| {
                d.child(
                    div()
                        .id(id("approval-judge"))
                        .aria_label(SharedString::from(judgement.clone()))
                        .text_size(px(12.5))
                        .child(SharedString::from(judgement))
                        .test_support(),
                )
            })
            .when_some(card.why.clone(), |d, why| {
                d.child(
                    div()
                        .id(id("approval-why"))
                        .text_size(px(12.5))
                        .text_color(p.mute)
                        .child(SharedString::from(why))
                        .test_support(),
                )
            })
            .when_some(card.source.clone(), |d, source| {
                d.child(
                    div()
                        .id(id("approval-source"))
                        .mono(11.)
                        .text_color(p.faint)
                        .child(SharedString::from(source))
                        .test_support(),
                )
            })
            .when_some(self.errors.get(&number).cloned(), |d, error| {
                d.child(
                    div()
                        .id(id("approval-error"))
                        .text_size(px(12.5))
                        .text_color(p.fail)
                        .child(error)
                        .test_support(),
                )
            });
        // A glow round the card the user was sent to, which fades in and out.
        div()
            .id(id("approval"))
            .p(space::XS)
            .rounded(px(14.))
            .bg(p.wait.opacity(0.28 * lit))
            .child(
                Card::new()
                    .attention()
                    .when(card.base, Card::alarm)
                    .child(body)
                    .action(button("deny", "Deny", Answer::Deny, false))
                    .action(button("allow", "Allow", Answer::Once, true)),
            )
            .test_support()
            .into_any_element()
    }
}

impl Render for Waiting {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let at = cx.background_executor().now();
        self.list.sync(
            self.cards.iter().map(|c| (c.number.to_string(), c.clone())),
            at,
            cx.reduce_motion(),
        );
        let p = palette(cx);
        let present = !self.list.is_empty();
        let listed: Vec<(String, ApprovalCard, Placement)> = self
            .list
            .rows()
            .map(|row| (row.key.to_owned(), row.value.clone(), row.placement))
            .collect();
        let cards: Vec<AnyElement> = listed
            .into_iter()
            .map(|(key, card, placement)| {
                let content = self.card(&card, p, window, cx);
                Item::new(
                    ElementId::Name(format!("waiting-item-{key}").into()),
                    placement,
                    content,
                )
                .gap(space::SM)
                .into_any_element()
            })
            .collect();
        let section = div()
            .id("rail-waiting")
            .flex()
            .flex_col()
            .gap(space::SM)
            .child(
                div()
                    .id("rail-waiting-title")
                    .aria_label("Waiting for you")
                    .child(section_label("Waiting for you", cx))
                    .test_support(),
            )
            .children(cards)
            .child(div().h(px(1.)).bg(p.line).mx(px(6.)).my(px(8.)))
            .test_support();
        presence("rail-waiting-presence", present, section)
    }
}

#[cfg(test)]
mod tests {
    use super::sentences;

    #[test]
    fn two_reasons_are_two_sentences_with_no_doubled_word() {
        let said = sentences([
            "it copies .env into every workspace, which matches a sensitive name (.env): only the \
             user chooses to spread it",
            "it changes the setup that prepares the project's new workspaces",
        ]);
        assert_eq!(
            said,
            "It copies .env into every workspace, which matches a sensitive name (.env): only the \
             user chooses to spread it. It changes the setup that prepares the project's new \
             workspaces."
        );
        assert!(!said.contains("it it"));
    }

    #[test]
    fn a_reason_that_ends_a_sentence_keeps_its_mark_and_an_empty_one_is_left_out() {
        assert_eq!(
            sentences([
                "it is the line it sets, pnpm test: not a command.",
                "  ",
                "why?"
            ]),
            "It is the line it sets, pnpm test: not a command. Why?"
        );
        assert_eq!(sentences(Vec::<&str>::new()), "");
        assert_eq!(sentences(["émile asked"]), "Émile asked.");
    }
}
