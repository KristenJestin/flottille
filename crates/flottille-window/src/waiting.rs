//! The rail's "Waiting for you" section, pinned at the rail's bottom: each request that reached
//! the user is a compact card with Details, Deny and Allow. The conversation keeps one neutral
//! line per request, which lights its card here; an answered request leaves the rail and its line
//! says the outcome. The section folds away when nothing waits: it is where the page says, once,
//! that the user is needed.
//!
//! The text of a card is what the daemon gave (`says`, the class, the judge's reason, the reasons
//! it was held, who asked); the window only joins the reasons into sentences.

use crate::state::{AgentState, StateIcon, Status};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use std::f32::consts::PI;

use gpui_design_system::color::mix;
use gpui_design_system::motion::PANEL;
use gpui_design_system::{
    Button, Icon, Item, Listing, Palette, Placement, Type, fade, hoverable, icon, palette,
    presence, space, turning,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Task,
    TestSupportExt as _, Transformation, WeakEntity, Window, div, px, radians,
};

use crate::actions::Answer;
use crate::commands::spaced;
use crate::runs::rail_label;
use crate::session::{ApprovalCard, SessionPage};

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
    /// The cards whose details are unfolded.
    open: HashSet<u64>,
    /// The pulsing dot of a request to judge.
    dot: Entity<StateIcon>,
    scroll: ScrollHandle,
    _lit: Option<Task<()>>,
    renders: usize,
}

impl Waiting {
    pub fn new(page: WeakEntity<SessionPage>, cx: &mut Context<Self>) -> Self {
        let status = cx.new(|_| Status::new(AgentState::NeedsYou));
        Self {
            page,
            cards: Vec::new(),
            list: Listing::default(),
            busy: HashSet::new(),
            errors: HashMap::new(),
            pointed: None,
            open: HashSet::new(),
            dot: cx.new(|cx| StateIcon::new(status, cx)),
            scroll: ScrollHandle::new(),
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
        self.open.retain(|n| cards.iter().any(|c| c.number == *n));
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

    /// Lights a card for a moment, scrolled into sight: the user was sent to it from the
    /// conversation.
    pub fn point_at(&mut self, number: u64, cx: &mut Context<Self>) {
        self.pointed = Some(number);
        if let Some(at) = self.cards.iter().position(|c| c.number == number) {
            self.scroll.scroll_to_item(at);
        }
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

    /// Opens or folds the rest of a card: the judge's words, why it was held, who asked.
    pub fn toggle_details(&mut self, number: u64, cx: &mut Context<Self>) {
        if !self.open.remove(&number) {
            self.open.insert(number);
        }
        cx.notify();
    }

    /// A compact card: what the call does and its number, the call in one line, its class and the
    /// reason in one line, then Details, Deny and Allow. A base request stands apart by its
    /// tag and its still dot, in the same attention tone: it failed nothing.
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
        let open = self.open.contains(&number);
        let lit = fade(
            id("approval-lit"),
            self.pointed == Some(number),
            PANEL,
            window,
            cx,
        );
        let turn = turning(id("approval-details"), open, window, cx);
        let button = |part: &str, label: &str, answer: Answer, primary: bool| {
            let button = match primary {
                true => Button::primary(id(part), label.to_owned()),
                false => Button::quiet(id(part), label.to_owned()),
            };
            button
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| this.answer(number, answer, cx)))
        };
        let tones = held_tones(p);
        let class = match card.base {
            true => "Always asks",
            false => "To judge",
        };
        let ink = tones.ink;
        // The reason in one line: the judge's when one was asked, else the first reason it was
        // held.
        let reason = card
            .judgement
            .clone()
            .or_else(|| {
                card.why.as_deref().map(|why| {
                    why.split_inclusive(". ")
                        .next()
                        .unwrap_or(why)
                        .trim()
                        .to_owned()
                })
            })
            .unwrap_or_default();
        let title = card
            .asked
            .clone()
            .unwrap_or_else(|| format!("Request #{number}"));
        // The call; an action, which changes the project, says it in words instead.
        let (line, from_says) = match (card.kind.as_str(), &card.call) {
            ("call", Some(call)) => (Some(call.clone()), false),
            _ => (card.says.first().cloned(), true),
        };
        let one_line = |d: gpui_kit::Stateful<gpui_kit::Div>| {
            d.min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
        };
        let head = div()
            .flex()
            .items_center()
            .gap(space::SM)
            .child(
                div()
                    .flex_none()
                    .w(px(16.))
                    .flex()
                    .justify_center()
                    .child(self.mark(card.base, p)),
            )
            .child(
                one_line(div().id(id("approval-title")))
                    .flex_1()
                    .text_size(px(13.))
                    .semibold()
                    .aria_label(SharedString::from(title.clone()))
                    .child(SharedString::from(title))
                    .test_support(),
            )
            .child(
                div()
                    .flex_none()
                    .mono(11.)
                    .text_color(p.faint)
                    .child(SharedString::from(format!("#{number}"))),
            );
        let details = div()
            .id(id("approval-more"))
            .pt(space::XS)
            .flex()
            .flex_col()
            .gap(space::SM)
            .text_size(px(12.))
            .text_color(p.mute)
            .children(
                card.says
                    .iter()
                    .enumerate()
                    .skip(usize::from(from_says))
                    .map(|(i, said)| {
                        div()
                            .id(ElementId::Name(
                                format!("approval-says-{number}-{i}").into(),
                            ))
                            .child(SharedString::from(said.clone()))
                            .test_support()
                    }),
            )
            .when(card.base, |d| {
                let says = card.always_says();
                d.child(
                    div()
                        .id(id("approval-base"))
                        .aria_label(says)
                        .text_color(p.fail)
                        .child(says)
                        .test_support(),
                )
            })
            .when_some(card.judgement.clone(), |d, judgement| {
                d.child(
                    div()
                        .id(id("approval-judge"))
                        .aria_label(SharedString::from(judgement.clone()))
                        .child(SharedString::from(judgement))
                        .test_support(),
                )
            })
            .when_some(card.why.clone(), |d, why| {
                d.child(
                    div()
                        .id(id("approval-why"))
                        .aria_label(SharedString::from(why.clone()))
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
            });
        let toggle = {
            let row = div()
                .id(id("approval-details"))
                .aria_expanded(open)
                .aria_label("Details")
                .flex()
                .items_center()
                .gap(space::XS)
                .text_size(px(12.))
                .cursor_pointer()
                .child("Details")
                .child(
                    icon(Icon::Chevron)
                        .size(px(11.))
                        .text_color(p.faint)
                        .with_transformation(Transformation::rotate(radians(PI * (1.0 - turn)))),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_details(number, cx)));
            hoverable(row, move |d, t| d.text_color(mix(p.faint, p.text, t)))
        };
        let body = div()
            .id(id("approval-card"))
            .flex()
            .flex_col()
            .px(space::MD)
            .py(space::SM)
            .rounded(px(12.))
            .bg(p.card)
            .border_1()
            .border_color(tones.card_edge)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space::XS)
                    .child(head)
                    // The whole call, wrapping over as many lines as it takes: what is allowed is
                    // read before it is allowed, never guessed from its start.
                    .children(line.map(|line| {
                        div()
                            .id(match from_says {
                                true => ElementId::Name(format!("approval-says-{number}-0").into()),
                                false => id("approval-call"),
                            })
                            .min_w_0()
                            .whitespace_normal()
                            .mono(11.5)
                            .text_color(p.mute)
                            .aria_label(SharedString::from(line.clone()))
                            .child(SharedString::from(line))
                            .test_support()
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space::SM)
                            .min_w_0()
                            .child(
                                div()
                                    .id(id("approval-class"))
                                    .flex_none()
                                    .aria_label(class)
                                    .mono(10.)
                                    .medium()
                                    .text_color(ink)
                                    .child(class.to_uppercase())
                                    .test_support(),
                            )
                            .child(
                                one_line(div().id(id("approval-reason")))
                                    .text_size(px(12.))
                                    .text_color(p.faint)
                                    .aria_label(SharedString::from(reason.clone()))
                                    .child(SharedString::from(reason))
                                    .test_support(),
                            ),
                    ),
            )
            .child(presence(id("approval-more-block"), open, details).space(space::XS))
            .children(self.errors.get(&number).cloned().map(|error| {
                spaced(
                    space::XS,
                    div()
                        .id(id("approval-error"))
                        .text_size(px(12.))
                        .text_color(p.fail)
                        .child(error)
                        .test_support(),
                )
            }))
            .child(spaced(
                space::XS,
                div()
                    .pt(space::XS)
                    .flex()
                    .items_center()
                    .gap(space::SM)
                    .child(div().flex_1().child(toggle))
                    .child(button("deny", "Deny", Answer::Deny, false))
                    .child(match card.proposes_a_connection() {
                        // What it would write is read in its dialog before it is accepted.
                        true => Button::primary(id("open-request"), "Open")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.page
                                    .update(cx, |page, cx| page.open_connection_request(number, cx))
                                    .ok();
                            })),
                        false => button("allow", "Allow", Answer::Once, true),
                    }),
            ));
        // A glow round the card the user was sent to, which fades in and out.
        div()
            .id(id("approval"))
            .p(space::XS)
            .rounded(px(14.))
            .bg(p.wait.opacity(0.28 * lit))
            .child(body)
            .test_support()
            .into_any_element()
    }

    /// The card's mark: the pulsing dot of what waits for the user, or the still one of a base request.
    fn mark(&self, base: bool, p: Palette) -> AnyElement {
        match base {
            true => div()
                .size(px(8.))
                .rounded_full()
                .bg(held_tones(p).ink)
                .into_any_element(),
            false => self.dot.clone().into_any_element(),
        }
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
                .rising()
                .into_any_element()
            })
            .collect();
        let count = self.cards.len();
        let section = div()
            .id("rail-waiting")
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(p.line)
            .pt(space::SM)
            .child(
                div()
                    .id("rail-waiting-title")
                    .aria_label("Waiting for you")
                    .child(rail_label(
                        "Waiting for you",
                        (count > 0).then(|| {
                            div()
                                .mono(11.)
                                .medium()
                                .text_color(p.wait)
                                .child(SharedString::from(count.to_string()))
                                .into_any_element()
                        }),
                        cx,
                    ))
                    .test_support(),
            )
            .child(
                div()
                    .id("rail-waiting-cards")
                    .max_h(px(WAITING_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .children(cards),
            )
            .test_support();
        presence("rail-waiting-presence", present, section)
    }
}

/// The most the cards take of the rail's height before they scroll.
const WAITING_HEIGHT: f32 = 420.;

/// The tones of a request held for the user, on the rail's card and on the conversation's line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeldTones {
    /// The dot and the class tag.
    pub ink: gpui_kit::Hsla,
    /// The card's border, over the card.
    pub card_edge: gpui_kit::Hsla,
    /// The conversation line's border once pointed at.
    pub line_edge: gpui_kit::Hsla,
}

/// A request that waits for the user is attention, a base one too: nothing failed. The error
/// ink is for what was refused or failed.
pub(crate) fn held_tones(p: Palette) -> HeldTones {
    HeldTones {
        ink: p.wait,
        card_edge: mix(p.wait, p.card, 0.7),
        line_edge: p.wait_line,
    }
}

/// The ink of the one-line record a decided request folds into: a denial is a refusal.
pub(crate) fn record_ink(decision: Option<&str>, p: Palette) -> gpui_kit::Hsla {
    match decision {
        Some("deny") => p.fail,
        _ => p.faint,
    }
}

#[cfg(test)]
mod tests {
    use super::{HeldTones, held_tones, record_ink, sentences};
    use gpui_design_system::{Palette, theme::Shade};

    fn palettes() -> [Palette; 2] {
        [Palette::own(Shade::Light), Palette::own(Shade::Dark)]
    }

    #[test]
    fn a_request_waiting_for_the_user_is_not_drawn_as_an_error() {
        for p in palettes() {
            let HeldTones {
                ink,
                card_edge,
                line_edge,
            } = held_tones(p);
            assert_eq!(ink, p.wait, "the dot and the tag");
            assert_ne!(ink, p.fail);
            assert_ne!(
                card_edge,
                gpui_design_system::color::mix(p.fail, p.card, 0.6)
            );
            assert_ne!(line_edge, p.fail, "the line's border");
        }
    }

    #[test]
    fn the_rail_card_and_the_conversation_line_agree() {
        for p in palettes() {
            assert_eq!(held_tones(p).line_edge, p.wait_line);
            assert_eq!(held_tones(p).ink, p.wait);
        }
    }

    #[test]
    fn a_denied_request_is_drawn_as_refused() {
        for p in palettes() {
            assert_eq!(record_ink(Some("deny"), p), p.fail);
            assert_ne!(record_ink(Some("once"), p), p.fail);
            assert_ne!(record_ink(None, p), p.fail);
        }
    }

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
